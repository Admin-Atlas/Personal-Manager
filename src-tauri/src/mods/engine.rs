// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The sealed engine (Mods spec §1.4, "What PR-1 ships from this"): what keeps a mod's sandboxed
//! frame off the network, and any document that isn't PM's out of a PM window's top frame. Every
//! mitigation here was measured closing its leak on all three engines by the step 0 spike; the ones
//! that measured ineffective (macOS `proxyConfigurations`, Windows `--force-webrtc-ip-handling-policy`)
//! are left out.
//!
//! This file names nothing in PM's crate, so the sealed-frame regression app (`tests/sealed-frame`)
//! can include it with `#[path]` and measure exactly what ships. PM's wiring (its state, its window
//! builders) is `mods/mod.rs`.
//!
//! - [`Sinkhole`]: a loopback listener PM's webviews use as their proxy, which drops every
//!   connection. `<link rel=preconnect>` opens TCP to any host despite the CSP, and the proxy is what
//!   stopped it (Linux, Windows). PM's own page makes no network requests (everything goes through
//!   Rust or the `tauri`, `ipc` and `pmmod` schemes), so a dead proxy costs it nothing.
//! - [`windows_browser_args`]: WebView2's args pointing at the sinkhole, with wry's defaults kept.
//! - [`webkitgtk::seal`] and [`wkwebview::seal`]: the engine switches, each reporting whether it took.
//!   On macOS that includes [`content_rules_json`], a content rule list that blocks every http, https,
//!   ws and wss load: WKWebView has no sinkhole, and this is its network wall.
//! - [`allow_navigation`]: the navigation guard's decision.
//! - [`top_frame_script`] and [`WEBRTC_OFF_SCRIPT`]: the two init scripts.
//! - [`seal_builder`] and [`apply_switches`]: the wiring that puts all of the above on a window,
//!   before and after it is built. PM's `mods::build_window` and the regression app both call them,
//!   so the app measures the wiring that ships too.
//! - [`verdict`]: whether every switch took. If one didn't, the engine counts as unsealed.
//!   [`unreported`] says whether it is final yet.
//! - [`build_retrying`] and [`Sinkhole::bind_preferring`]: what keeps a relaunch on Windows from
//!   failing on browser args that differ from the ones the previous run's WebView2 is still using,
//!   and [`hidden_until_built`], which keeps a refused try off the screen.
//!
//! **The guard fails open on a URL Rust can't parse.** tauri-runtime-wry 2.12 calls the guard only
//! after `url.parse()`, and allows the navigation when that fails (`.unwrap_or(true)`). Some
//! engines accept hosts the `url` crate refuses (`xn--1ug.example`, an IDNA error): WebView2 and
//! Fedora's WebKitGTK 2.54 do, and the guard never sees them there. What stops a frame anyway, on
//! every engine: main's CSP `frame-src`, which names only the mod origins and which WebKit enforces
//! on a frame's own navigation (measured on WebKitGTK 2.54: a sandboxed frame's `location.href` to
//! http and to `data:` was refused, and loaded once `frame-src` named it); then the sinkhole on
//! Linux and Windows and the content rule list on macOS, which never ask Rust. The sealed-frame job
//! self-navigates to such a host on all three engines, but reaches this path only where the engine
//! accepts it: the WebKitGTK on ubuntu-latest and the WebKit on macos-latest refuse it as an
//! invalid URL, so there the navigation fails before Tauri's guard.

use std::net::{Ipv4Addr, TcpListener};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tauri::utils::config::WindowConfig;
use tauri::webview::DownloadEvent;
use tauri::{AppHandle, Url, WebviewWindow, WebviewWindowBuilder, Wry};

/// The three engines PM ships on. Each serves PM and mods from different origins and lets the
/// navigation guard see different things.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Platform {
    /// WebKitGTK.
    Linux,
    /// WKWebView.
    MacOs,
    /// WebView2.
    Windows,
}

impl Platform {
    pub const fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Linux
        }
    }

    /// The origin Tauri serves PM's own pages from (`tauri::protocol::origin`): its custom scheme on
    /// WebKit, and wry's `http://<scheme>.localhost` stand-in on WebView2. The `https` form would be
    /// Tauri's with `useHttpsScheme` on, which PM leaves off (a test in `mods/mod.rs` pins that).
    pub const fn app_origin(self) -> &'static str {
        match self {
            Self::Windows => "http://tauri.localhost",
            Self::Linux | Self::MacOs => "tauri://localhost",
        }
    }

    /// The origin every mod is served from: the `pmmod` scheme, under the same rule.
    pub const fn mod_origin(self) -> &'static str {
        match self {
            Self::Windows => "http://pmmod.localhost",
            Self::Linux | Self::MacOs => "pmmod://localhost",
        }
    }

    /// Whether the navigation guard is asked about subframes as well as the top level. WebKit asks
    /// about both (and can't say which it is asking about); WebView2 asks about the top level only.
    pub const fn guard_sees_subframes(self) -> bool {
        !matches!(self, Self::Windows)
    }

    /// Whether this engine is sealed through the sinkhole proxy. WKWebView ignored the proxy in
    /// every form the spike tried, so macOS has feature switches instead and no sinkhole at all.
    pub const fn uses_sinkhole(self) -> bool {
        !matches!(self, Self::MacOs)
    }

    /// The switches that must all take for this engine to count as sealed. Only switches whose
    /// report can say "didn't take" belong here: WebKitGTK's DNS-prefetching setting can't (see
    /// [`webkitgtk::seal`]), so it isn't one.
    pub const fn expected_switches(self) -> &'static [&'static str] {
        match self {
            Self::Linux => &[SINKHOLE, LINUX_PROXY, LINUX_WEBRTC_OFF],
            Self::Windows => &[SINKHOLE, WINDOWS_PROXY_ARGS],
            Self::MacOs => &[
                MACOS_PEER_CONNECTION_OFF,
                MACOS_FEATURES[0],
                MACOS_FEATURES[1],
                MACOS_FEATURES[2],
                MACOS_FEATURES[3],
                MACOS_NETWORK_RULES,
            ],
        }
    }
}

// ---- The switches, and whether they took -------------------------------------------------------

/// The sinkhole is listening.
pub const SINKHOLE: &str = "sinkhole";
/// WebKitGTK: the webview's network proxy points at the sinkhole.
pub const LINUX_PROXY: &str = "webkitgtk.proxy";
/// WebKitGTK: `enable-webrtc` is off. Fedora's and Ubuntu's builds have no WebRTC at all; this
/// keeps it off on a build that has.
pub const LINUX_WEBRTC_OFF: &str = "webkitgtk.enable-webrtc-off";
/// WebView2: the browser args carry the sinkhole proxy, and nothing outside PM overrides them.
pub const WINDOWS_PROXY_ARGS: &str = "webview2.proxy-args";
/// WKWebView: `-[WKPreferences _setPeerConnectionEnabled:NO]` (SPI).
pub const MACOS_PEER_CONNECTION_OFF: &str = "wkwebview.peer-connection-off";
/// WKWebView: the preference features switched off through `-[WKPreferences _setEnabled:forFeature:]`
/// (SPI), by the keys `+[WKPreferences _features]` lists them under. These four closed preconnect.
pub const MACOS_FEATURES: [&str; 4] = [
    "LinkPreconnect",
    "LinkPreconnectEarlyHintsEnabled",
    "LinkPrefetchEnabled",
    MACOS_DNS_PREFETCHING,
];
/// WKWebView's DNS prefetching. The step 0 spike found it among `_features` on macos-14, but the
/// WebKit on macos-latest doesn't list it (pre-flight CI run 42023117805), because WebKit removed
/// the preference (commit 634cc1e388c3, February 2025; no WebKit release tag from 7622 on has it).
/// That change also dropped the implicit prefetching of a link's and a hovered link's host, stopped
/// reading `X-DNS-Prefetch-Control`, and left the preference's SPI as stubs: an empty
/// `-[WKPreferences _setDNSPrefetchingEnabled:]` and a `_dnsPrefetchingEnabled` that always says NO.
/// The one DNS prefetch left, `<link rel=dns-prefetch>`, got a preference of its own in the same
/// commit, [`MACOS_LINK_DNS_PREFETCH`], which every release tag from 7622.1.7 through 7624 has.
/// WebKit removed that one too later (no 7625 tag has it), by which time `<link rel=dns-prefetch>`
/// met the content rule lists instead (commit ce84da3fd2d6, June 2025, in every tag from 7622.1.21
/// on). Read from WebKit's source at those tags.
///
/// So the switch takes through whichever of these this WebKit has, and nothing else counts:
/// - `DNSPrefetchingEnabled` itself, where `_features` lists it (macos-14; the 7620 and 7621 tags
///   have it);
/// - else [`MACOS_LINK_DNS_PREFETCH`], where `_features` lists it (7622 to 7624), switched off by
///   the same route and counted only if `_isEnabledForFeature:` reads it back off;
/// - else, where a `_features` list names neither, the setter (see [`DnsPrefetchSpi`]): taken if
///   the getter follows it on and off (a working preference `_features` doesn't list), or gone if
///   setter and getter act as those stubs do (7625 on). Then [`MACOS_NETWORK_RULES`], already a
///   switch of the seal, is what closes DNS prefetching.
///
/// A failure's detail, and a gone one's, names the listed features whose keys mention DNS or
/// prefetching, so a run's report says what that WebKit has. A mod's documents still carry
/// `X-DNS-Prefetch-Control: off` (headers.rs), which only a WebKit from before the removal reads,
/// and only for the implicit prefetching.
pub const MACOS_DNS_PREFETCHING: &str = "DNSPrefetchingEnabled";
/// `<link rel=dns-prefetch>`'s own preference, the `_features` key DNS prefetching is switched off
/// by where `DNSPrefetchingEnabled` isn't listed (see [`MACOS_DNS_PREFETCHING`]). Its outcome is
/// reported under [`MACOS_DNS_PREFETCHING`]'s name, which the seal expects.
pub const MACOS_LINK_DNS_PREFETCH: &str = "LinkDNSPrefetchEnabled";
/// WKWebView: the content rule list from [`content_rules_json`] compiled and added to the webview.
pub const MACOS_NETWORK_RULES: &str = "wkwebview.content-rule-list";

/// The name the rule list is compiled under in WebKit's store. Compiling again replaces it.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub const CONTENT_RULE_LIST_ID: &str = "org.itsatlas.pm.no-network";

/// WKWebView's content rule list: block every http, https, ws and wss load in a PM webview, top
/// level and frames alike, as the sinkhole does elsewhere (PM's own page makes none; its schemes are
/// `tauri`, `ipc` and `pmmod`). WebKit applies it itself, so it holds for a URL Rust can't parse.
/// A dev build lets Vite's server through, as the proxy does. WebKit's rule regexes have no `|`, so
/// each scheme and host is a rule of its own; the `ignore-previous-rules` ones come last so they
/// override the blocks.
///
/// WebKit matches a filter against the whole URL, userinfo included, so an exemption names the
/// entire authority: the host, an optional port, then the path's `/`. A prefix such as
/// `localhost[:/]` would also exempt `http://localhost:1@evil.example/`, whose host is
/// evil.example. The proxy's ignore list (Linux) matches the real host, so this keeps the two alike.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub fn content_rules_json(is_dev: bool) -> String {
    let mut rules: Vec<String> = ["^https?://", "^wss?://"]
        .iter()
        .map(|filter| rule(filter, "block"))
        .collect();
    if is_dev {
        for scheme in ["https?", "wss?"] {
            for host in ["localhost", "127\\\\.0\\\\.0\\\\.1"] {
                rules.push(rule(
                    &format!("^{scheme}://{host}(:[0-9]+)?/"),
                    "ignore-previous-rules",
                ));
            }
        }
    }
    format!("[{}]", rules.join(","))
}

/// One rule. The filters are fixed strings above (no quote, already JSON-escaped).
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn rule(url_filter: &str, action: &str) -> String {
    format!(r#"{{"trigger":{{"url-filter":"{url_filter}"}},"action":{{"type":"{action}"}}}}"#)
}

/// What happened when PM tried one switch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchOutcome {
    pub name: &'static str,
    pub took: bool,
    /// Why it didn't take, or, for one that counts as taken with nothing left to switch, why there
    /// is nothing, or, for one switched by another name than its own, which; empty when it was
    /// switched by its own name.
    pub detail: String,
}

impl SwitchOutcome {
    pub fn took(name: &'static str) -> Self {
        Self {
            name,
            took: true,
            detail: String::new(),
        }
    }

    /// Taken, switched by another name than its own (`how`).
    #[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
    pub fn took_as(name: &'static str, how: impl Into<String>) -> Self {
        Self {
            name,
            took: true,
            detail: how.into(),
        }
    }

    /// Taken with nothing to switch: what it would switch off is gone from this engine (`why`).
    #[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
    pub fn gone(name: &'static str, why: impl Into<String>) -> Self {
        Self {
            name,
            took: true,
            detail: why.into(),
        }
    }

    pub fn failed(name: &'static str, detail: impl Into<String>) -> Self {
        Self {
            name,
            took: false,
            detail: detail.into(),
        }
    }
}

/// What `+[WKPreferences _features]` gave (the macOS feature switches): an outcome for each of
/// [`MACOS_FEATURES`] it lists, by slot, what [`MACOS_LINK_DNS_PREFETCH`] read back if it lists
/// that, and every listed key that mentions DNS or prefetching.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
#[derive(Debug, Default)]
pub struct ListedFeatures {
    pub outcomes: [Option<SwitchOutcome>; MACOS_FEATURES.len()],
    pub link_dns: Option<FeatureRead>,
    pub dns_named: Vec<String>,
}

/// What `-[WKPreferences _isEnabledForFeature:]` said of a feature just switched off with
/// `_setEnabled:NO forFeature:`.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureRead {
    /// It reads off.
    Off,
    /// It still reads on.
    StillOn,
    /// WebKit has no `_isEnabledForFeature:` to ask.
    NoGetter,
}

/// What `-[WKPreferences _setDNSPrefetchingEnabled:]` did, tried only where `_features` lists
/// neither DNS-prefetching key. The setting is switched on and read, then off and read, because a
/// getter that always reads off proves nothing by itself: one that follows both is a working
/// preference, now off, and one that reads off both times is the stub WebKit left when it removed
/// the preference (see [`MACOS_DNS_PREFETCHING`]), as WebKitGTK's deprecated setting is since 2.48.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DnsPrefetchSpi {
    /// WebKit has no such setter.
    NoSetter,
    /// It has the setter (switched off all the same) but no `_dnsPrefetchingEnabled` to read back.
    NoGetter,
    /// The getter read off with the setting switched on, and off again: the removed preference's
    /// stubs (an empty setter, a getter that returns NO), so this setter has nothing to switch.
    Stubbed,
    /// The getter read on after the setting was switched off.
    StillOn,
    /// The getter followed it on, then off.
    Off,
}

/// Why DNS prefetching counts as taken where its preference is gone (see [`MACOS_DNS_PREFETCHING`]).
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub const DNS_PREFETCHING_GONE: &str = "this WebKit has no DNS-prefetching preference \
     (+[WKPreferences _features] lists neither DNSPrefetchingEnabled nor LinkDNSPrefetchEnabled, \
     -[WKPreferences _setDNSPrefetchingEnabled:] does nothing and _dnsPrefetchingEnabled always \
     reads off); the one DNS prefetch it has, <link rel=dns-prefetch>, meets the content rule list";

/// How DNS prefetching was switched where `_features` lists `<link rel=dns-prefetch>`'s preference.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub const LINK_DNS_PREFETCH_OFF: &str = "switched off as LinkDNSPrefetchEnabled, the \
     <link rel=dns-prefetch> preference this WebKit has in its place";

/// The four feature outcomes, in [`MACOS_FEATURES`]' order. `listed` is what the `_features` route
/// found, or why it had no list to go by; `dns_spi` tries DNS prefetching's own setter, and is called
/// only when that route found neither `DNSPrefetchingEnabled` nor [`MACOS_LINK_DNS_PREFETCH`].
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub fn macos_feature_outcomes(
    listed: Result<ListedFeatures, &str>,
    dns_spi: impl FnOnce() -> DnsPrefetchSpi,
) -> Vec<SwitchOutcome> {
    let (found, link_dns, why, dns_named) = match listed {
        Ok(listed) => (
            listed.outcomes,
            listed.link_dns,
            "WebKit lists no such feature",
            Some(listed.dns_named),
        ),
        Err(why) => (Default::default(), None, why, None),
    };
    let mut dns_spi = Some(dns_spi);
    let mut out = Vec::with_capacity(MACOS_FEATURES.len());
    for (&name, outcome) in MACOS_FEATURES.iter().zip(found) {
        if name != MACOS_DNS_PREFETCHING {
            out.push(outcome.unwrap_or_else(|| SwitchOutcome::failed(name, why)));
            continue;
        }
        // No WebKit tag lists both keys; one that did would have both switched off, and both count.
        out.push(match (outcome, link_dns) {
            (Some(outcome), _) if !outcome.took => outcome,
            (_, Some(read)) => link_dns_prefetch_outcome(read),
            (Some(outcome), None) => outcome,
            (None, None) => {
                let spi = dns_spi.take().map_or(DnsPrefetchSpi::NoSetter, |f| f());
                dns_prefetch_outcome(why, spi, dns_named.as_deref())
            }
        });
    }
    out
}

/// The DNS-prefetching switch where `_features` lists [`MACOS_LINK_DNS_PREFETCH`] (`read`, what it
/// read back once switched off): taken only if it reads off. Unlike the other listed features, a
/// missing getter doesn't count, as DNS prefetching's own setter doesn't (see [`DnsPrefetchSpi`]).
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub fn link_dns_prefetch_outcome(read: FeatureRead) -> SwitchOutcome {
    let failed = |why: &str| {
        SwitchOutcome::failed(
            MACOS_DNS_PREFETCHING,
            format!("{MACOS_LINK_DNS_PREFETCH} {why}"),
        )
    };
    match read {
        FeatureRead::Off => SwitchOutcome::took_as(MACOS_DNS_PREFETCHING, LINK_DNS_PREFETCH_OFF),
        FeatureRead::StillOn => failed("still on after switching it off"),
        FeatureRead::NoGetter => failed(
            "switched off, but WebKit has no -[WKPreferences _isEnabledForFeature:] to read it \
             back",
        ),
    }
}

/// How many of the keys that mention DNS or prefetching a failure's detail names, at most.
const DNS_KEYS_NAMED: usize = 16;

/// Whether a `_features` key mentions DNS or prefetching, in any case.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub fn names_dns_or_prefetch(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("dns") || key.contains("prefetch")
}

/// The DNS-prefetching switch where `_features` listed neither of its keys, or had no list (`why`):
/// taken if its own setter switched it off (`spi`), and gone if setter and getter are the removed
/// preference's stubs and WebKit listed its features (`listed`), which leaves the content rule list
/// to close the DNS prefetching that is left. With no list, stubs don't count: nothing says
/// [`MACOS_LINK_DNS_PREFETCH`] isn't there to switch. A failure's detail, and a gone one's, says
/// what was missing and, when WebKit listed its features, which keys mention DNS or prefetching.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
pub fn dns_prefetch_outcome(
    why: &str,
    spi: DnsPrefetchSpi,
    listed: Option<&[String]>,
) -> SwitchOutcome {
    let setter = "-[WKPreferences _setDNSPrefetchingEnabled:]";
    let named = dns_keys_named(listed);
    let spi_says = match spi {
        DnsPrefetchSpi::Off => return SwitchOutcome::took(MACOS_DNS_PREFETCHING),
        DnsPrefetchSpi::Stubbed if listed.is_some() => {
            return SwitchOutcome::gone(
                MACOS_DNS_PREFETCHING,
                format!("{DNS_PREFETCHING_GONE}{named}"),
            )
        }
        DnsPrefetchSpi::Stubbed => format!(
            "{setter} does nothing and _dnsPrefetchingEnabled always reads off, but with no list \
             of features to go by, {MACOS_LINK_DNS_PREFETCH} may still be there to switch off"
        ),
        DnsPrefetchSpi::NoSetter => format!("WebKit has no {setter}"),
        DnsPrefetchSpi::NoGetter => format!(
            "WebKit has {setter} but no -[WKPreferences _dnsPrefetchingEnabled] to read it \
             back"
        ),
        DnsPrefetchSpi::StillOn => format!("still on after {setter} switched it off"),
    };
    SwitchOutcome::failed(MACOS_DNS_PREFETCHING, format!("{why}; {spi_says}{named}"))
}

/// The tail of a DNS-prefetching detail that names the listed keys that mention DNS or
/// prefetching (at most [`DNS_KEYS_NAMED`]); empty when there was no list.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
fn dns_keys_named(listed: Option<&[String]>) -> String {
    match listed {
        None => String::new(),
        Some([]) => "; no listed feature mentions DNS or prefetch".to_owned(),
        Some(keys) => {
            let shown: Vec<&str> = keys
                .iter()
                .take(DNS_KEYS_NAMED)
                .map(String::as_str)
                .collect();
            let more = keys.len() - shown.len();
            let more = if more > 0 {
                format!(" and {more} more")
            } else {
                String::new()
            };
            format!(
                "; the listed features that mention DNS or prefetch: {}{more}",
                shown.join(", ")
            )
        }
    }
}

/// Whether the engine is sealed, and if not, why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub sealed: bool,
    pub reasons: Vec<String>,
}

/// Fail closed: sealed only when every expected switch has reported and every report of it says it
/// took. A switch that never reported counts against the seal (it was never tried, or its report is
/// still on the way), and so does a failed report of a switch nobody expected, since that means the
/// expected list is out of step with what PM tries.
pub fn verdict(expected: &[&str], outcomes: &[SwitchOutcome]) -> Verdict {
    let mut reasons = Vec::new();
    for name in expected {
        let mut reported = false;
        for o in outcomes.iter().filter(|o| o.name == *name) {
            reported = true;
            if !o.took {
                reasons.push(format!("{name} didn't take: {}", o.detail));
            }
        }
        if !reported {
            reasons.push(format!("{name} was never applied"));
        }
    }
    for o in outcomes
        .iter()
        .filter(|o| !o.took && !expected.contains(&o.name))
    {
        reasons.push(format!("{} didn't take: {}", o.name, o.detail));
    }
    Verdict {
        sealed: reasons.is_empty(),
        reasons,
    }
}

/// The expected switches with no report yet. Until this is empty the [`verdict`] isn't final: on
/// macOS the content rule list reports from WebKit's completion handler, after `setup()` has
/// returned, so a verdict read straight after the build would always say it was never applied.
pub fn unreported<'a>(expected: &[&'a str], outcomes: &[SwitchOutcome]) -> Vec<&'a str> {
    expected
        .iter()
        .copied()
        .filter(|name| !outcomes.iter().any(|o| o.name == *name))
        .collect()
}

// ---- The sinkhole ------------------------------------------------------------------------------

/// The dead proxy: a loopback listener that accepts each connection and closes it at once, reading
/// nothing (so there is nothing to parse), and counts the attempts for the Developer tab. With it as
/// the webview's proxy, a connection the CSP couldn't stop reaches this socket and no further: the
/// remote host isn't even resolved. Neither engine falls back to a direct connection when a proxy it
/// was given explicitly fails.
pub struct Sinkhole {
    port: u16,
    attempts: Arc<AtomicU64>,
}

impl Sinkhole {
    /// Bind a loopback port and start the thread that drains it, for the life of the process. The
    /// port is `preferred` when that is free: the port the last run used, so that on Windows a
    /// relaunch usually gets the same browser args as the WebView2 the last run may still have
    /// running (see [`build_retrying`]). A port that is taken, or no port, means a fresh one.
    pub fn bind_preferring(preferred: Option<u16>) -> std::io::Result<Self> {
        let listener = preferred
            .and_then(|port| TcpListener::bind((Ipv4Addr::LOCALHOST, port)).ok())
            .map_or_else(|| TcpListener::bind((Ipv4Addr::LOCALHOST, 0)), Ok)?;
        let port = listener.local_addr()?.port();
        let attempts = Arc::new(AtomicU64::new(0));
        let counter = Arc::clone(&attempts);
        std::thread::Builder::new()
            .name("pm-sinkhole".into())
            .spawn(move || {
                for conn in listener.incoming() {
                    match conn {
                        // Dropping the stream closes it.
                        Ok(_stream) => {
                            counter.fetch_add(1, Ordering::Relaxed);
                        }
                        // Out of file descriptors and the like: wait rather than spin.
                        Err(_) => std::thread::sleep(Duration::from_millis(100)),
                    }
                }
            })?;
        Ok(Self { port, attempts })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Connections that reached the sinkhole. Any local process can connect to it too, so this
    /// counts attempts, not proof that each came from a webview.
    pub fn attempts(&self) -> u64 {
        self.attempts.load(Ordering::Relaxed)
    }
}

/// The proxy URL that points an engine at the sinkhole.
pub fn sinkhole_proxy_uri(port: u16) -> String {
    format!("http://127.0.0.1:{port}")
}

/// The port a remembered-port file names: one decimal number, nothing else, and not a system port
/// (anything at all could have written the file; a wrong number only means a fresh port).
pub fn parse_remembered_port(text: &str) -> Option<u16> {
    let text = text.trim();
    if text.is_empty() || !text.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    text.parse::<u16>().ok().filter(|port| *port >= 1024)
}

/// How long to wait between tries at building the main window on Windows. WebView2 refuses a
/// webview whose browser args differ from those of a WebView2 browser still running on the same
/// user-data folder, and the sinkhole's port is in the args: when PM relaunches itself (Tauri's
/// `restart` spawns the new process while the old one is still exiting) or is reopened at once, the
/// old browser lives on for a moment with the old port. It exits shortly after its host, so a few
/// seconds of retries see it gone. About eight seconds in all.
pub const WINDOWS_RETRY_DELAYS: [Duration; 7] = [
    Duration::from_millis(250),
    Duration::from_millis(500),
    Duration::from_millis(750),
    Duration::from_secs(1),
    Duration::from_millis(1500),
    Duration::from_secs(2),
    Duration::from_secs(2),
];

/// Build a window with `attempt`, retrying on Windows after each [`WINDOWS_RETRY_DELAYS`] wait
/// (`wait` sleeps; a test passes its own). Elsewhere a failure is returned at once: nothing there
/// refuses a window for a previous run's sake. The last error is the one returned.
pub fn build_retrying<T, E: std::fmt::Display>(
    platform: Platform,
    mut attempt: impl FnMut() -> Result<T, E>,
    mut wait: impl FnMut(Duration),
) -> Result<T, E> {
    let delays: &[Duration] = match platform {
        Platform::Windows => &WINDOWS_RETRY_DELAYS,
        Platform::Linux | Platform::MacOs => &[],
    };
    let mut result = attempt();
    for delay in delays {
        let Err(e) = &result else {
            break;
        };
        eprintln!(
            "webview: building the window failed ({e}); trying again in {} ms",
            delay.as_millis()
        );
        wait(*delay);
        result = attempt();
    }
    result
}

/// A window's config as it is built, hidden whatever it says, and whether to show the window once
/// it is built and its switches applied ([`apply_switches`]; on macOS that only starts the content
/// rule list's compile, so there the window is shown before the list is in place, and the engine is
/// unsealed until it reports). On Windows tao shows a window before wry makes its webview,
/// and a window whose webview WebView2 refused only asks to be destroyed (tao's `Drop` posts a
/// message), so each refused try of [`build_retrying`] would leave a blank window on screen until a
/// message pump ran: while PM waits between tries, none does. Built hidden, a refused try never
/// shows.
///
/// Nothing else changes, `maximized` included (read from tao 0.37.2): on Windows a hidden window
/// keeps its maximized flag and is maximized by the `show()` (`SW_MAXIMIZE`, which also activates
/// it); on macOS a frameless window's frame is set to the screen's visible frame, as when it is
/// built visible; on Linux GTK maximizes it at its first configure, when it is mapped. And `show()`
/// makes the calls building it visible made (`SW_SHOW`, `makeKeyAndOrderFront`, `show_all`). The
/// webview's own visibility doesn't follow the window config, so it needs no showing.
pub fn hidden_until_built(config: &WindowConfig) -> (WindowConfig, bool) {
    let mut hidden = config.clone();
    hidden.visible = false;
    (hidden, config.visible)
}

/// Hosts the WebKitGTK proxy leaves alone: none in a release build; in a dev build Vite's server,
/// which serves PM's page itself.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn proxy_ignore_hosts(is_dev: bool) -> &'static [&'static str] {
    if is_dev {
        &["localhost", "127.0.0.1"]
    } else {
        &[]
    }
}

/// wry's WebView2 args (`wry` `webview2/mod.rs`). Passing any args replaces them, so they are
/// restated: they turn off Edge's mini menu, its PDF UI and SmartScreen. wry adds to them only for
/// autoplay or its own proxy option, neither of which PM sets.
pub const WRY_DEFAULT_BROWSER_ARGS: &str =
    "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection";

/// WebView2's browser args for every PM window. They must be identical in every window: WebView2
/// refuses a second webview in the same user-data folder with different environment options, and all
/// PM's windows share one.
///
/// `<-loopback>` removes Chromium's implicit bypass for loopback and `*.localhost`, so even a
/// connection to 127.0.0.1 goes to the sinkhole (PM's own schemes are served before the network
/// stack, so they are unaffected). A dev build keeps that bypass, for Vite.
pub fn windows_browser_args(sinkhole_port: Option<u16>, is_dev: bool) -> String {
    let mut args = String::from(WRY_DEFAULT_BROWSER_ARGS);
    if let Some(port) = sinkhole_port {
        args.push_str(" --proxy-server=");
        args.push_str(&sinkhole_proxy_uri(port));
        if !is_dev {
            args.push_str(" --proxy-bypass-list=<-loopback>");
        }
    }
    args
}

/// WebView2 can't be asked whether the args took, so the outcome is whether the main window was
/// built with the proxy in them (a second set would have failed the build). Whether something
/// outside PM replaces them is [`webview2_args_override`]'s question, asked by PM at startup.
#[cfg_attr(any(target_os = "linux", target_os = "macos"), allow(dead_code))]
pub fn proxy_args_outcome(sinkhole_port: Option<u16>) -> SwitchOutcome {
    match sinkhole_port {
        Some(_) => SwitchOutcome::took(WINDOWS_PROXY_ARGS),
        None => SwitchOutcome::failed(WINDOWS_PROXY_ARGS, "there is no sinkhole to point it at"),
    }
}

/// The environment variable WebView2 takes browser args from in place of the ones an app passes.
pub const WEBVIEW2_ARGS_ENV: &str = "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS";

/// The registry key (under HKLM and HKCU) whose values set WebView2's browser args by policy, in
/// place of the app's. A value applies when it is named for the app: its executable's file name,
/// its AppUserModelID, or `*` for every app.
#[cfg_attr(not(windows), allow(dead_code))]
pub const WEBVIEW2_ARGS_POLICY_KEY: &str =
    "SOFTWARE\\Policies\\Microsoft\\Edge\\WebView2\\AdditionalBrowserArguments";

/// The policy value names that would apply to PM: its executable's file name and its identifier
/// (in case that is its AppUserModelID), each as given, and `*`.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
pub fn webview2_policy_value_names(exe_name: Option<&str>, identifier: &str) -> Vec<String> {
    let mut names: Vec<String> = exe_name
        .into_iter()
        .chain([identifier, "*"])
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
        .collect();
    names.dedup();
    names
}

/// Why WebView2 may be running without PM's args, or `None` when nothing replaces them. WebView2
/// lets [`WEBVIEW2_ARGS_ENV`] and a [`WEBVIEW2_ARGS_POLICY_KEY`] value override the args an app
/// passes, and PM can't read back the args it ended up with. So the variable being set at all, even
/// empty, and a policy value for PM (`policy` names where it was found) both count as overridden:
/// the switch fails and the engine stays unsealed, though the proxy may well be in place.
pub fn webview2_args_override(
    env: Option<&std::ffi::OsStr>,
    policy: Option<&str>,
) -> Option<String> {
    if env.is_some() {
        return Some(format!(
            "{WEBVIEW2_ARGS_ENV} is set, which replaces the browser args PM passes"
        ));
    }
    policy.map(|at| format!("a WebView2 policy sets the browser args ({at})"))
}

// ---- The navigation guard ----------------------------------------------------------------------

/// Whether a PM window may navigate to `url`: PM's own origin, the dev server in a dev build, and a
/// mod's URL only while `is_pending_entry` says it is a one-time entry URL a frame is about to load
/// (spec §1.4, defences 1 and 2).
pub fn allow_navigation(url: &Url, is_dev: bool, is_pending_entry: &dyn Fn(&Url) -> bool) -> bool {
    allow_navigation_on(Platform::current(), url, is_dev, is_pending_entry)
}

/// [`allow_navigation`] for a given engine, so every engine's table can be tested on any one.
pub fn allow_navigation_on(
    platform: Platform,
    url: &Url,
    is_dev: bool,
    is_pending_entry: &dyn Fn(&Url) -> bool,
) -> bool {
    // A frame given no src, or blanked, navigates to about:blank, and WebKit's guard can't tell a
    // frame from the top. The spike measured the frame sealed with it allowed; it has no content, and
    // its origin is its creator's. Nothing else under about: is allowed (about:srcdoc among them).
    if url.scheme() == "about" {
        return url.path() == "blank";
    }
    let Some(origin) = origin_of(url) else {
        return false;
    };
    if origin == platform.app_origin() {
        // Every load of PM's own page, a reload (the frozen-mod watchdog's) included.
        return true;
    }
    if is_dev && is_dev_server(url) {
        return true;
    }
    if origin == platform.mod_origin() {
        // WebView2 asks only about the top level, where a mod's page is never wanted.
        return platform.guard_sees_subframes() && is_pending_entry(url);
    }
    false
}

/// `scheme://host[:port]`, built by hand: `Url::origin` is opaque for non-special schemes such as
/// `tauri:` and `pmmod:`, which are the ones that matter here. `None` for a URL with no host.
fn origin_of(url: &Url) -> Option<String> {
    let host = url.host_str().filter(|h| !h.is_empty())?;
    Some(match url.port() {
        Some(port) => format!("{}://{host}:{port}", url.scheme()),
        None => format!("{}://{host}", url.scheme()),
    })
}

/// Vite's server, which serves PM's page in a dev build.
fn is_dev_server(url: &Url) -> bool {
    url.scheme() == "http" && matches!(url.host_str(), Some("localhost" | "127.0.0.1"))
}

// ---- The init scripts --------------------------------------------------------------------------

/// The all-frames init script (`initialization_script_for_all_frames`) that removes WebRTC from every
/// mod document. See the file for what it touches and why.
pub const WEBRTC_OFF_SCRIPT: &str = include_str!("webrtc_off.js");

const TOP_FRAME_JS: &str = include_str!("top_frame.js");

/// The top-frame init script (`initialization_script`) that stops any document that isn't PM's own
/// page. `top_frame.js` reads its one parameter; the origins are fixed strings, so no escaping.
pub fn top_frame_script(platform: Platform, is_dev: bool) -> String {
    format!(
        "(function (PM_TOP_FRAME) {{\n{TOP_FRAME_JS}\n}})({{\"origins\":[\"{}\"],\"dev\":{is_dev}}});",
        platform.app_origin()
    )
}

// ---- The wiring --------------------------------------------------------------------------------

/// Everything a sealed window gets before it is built: the browser args, the navigation guard, the
/// download refusal and both init scripts. `is_pending_entry` is asked only about a mod URL, and only
/// on an engine whose guard sees frames ([`allow_navigation_on`]).
pub fn seal_builder<'a>(
    builder: WebviewWindowBuilder<'a, Wry, AppHandle>,
    browser_args: &str,
    is_dev: bool,
    is_pending_entry: impl Fn(&Url) -> bool + Send + 'static,
) -> WebviewWindowBuilder<'a, Wry, AppHandle> {
    builder
        .additional_browser_args(browser_args)
        .on_navigation(move |url| {
            let allowed = allow_navigation(url, is_dev, &is_pending_entry);
            if !allowed {
                // The scheme only: a URL can carry anything.
                eprintln!("webview: refused a navigation to a {}: URL", url.scheme());
            }
            allowed
        })
        // PM downloads nothing through its webviews: files reach the user through Rust and the
        // save dialog. Refusing here also refuses a mod's download link.
        .on_download(|_, event| {
            if let DownloadEvent::Requested { url, .. } = event {
                eprintln!("webview: refused a download of a {}: URL", url.scheme());
            }
            false
        })
        .initialization_script(top_frame_script(Platform::current(), is_dev))
        .initialization_script_for_all_frames(WEBRTC_OFF_SCRIPT)
}

/// Apply the engine switches to a built window, and hand `done` what took. WebKitGTK's and
/// WKWebView's switches run on the main thread through `with_webview`, so `done` runs later, there.
/// `Err` when the webview couldn't be reached: `done` then never runs, and a switch that never
/// reports keeps the engine unsealed ([`verdict`]).
#[cfg(target_os = "linux")]
pub fn apply_switches(
    window: &WebviewWindow,
    sinkhole_port: Option<u16>,
    is_dev: bool,
    done: impl FnOnce(Vec<SwitchOutcome>) + Send + 'static,
) -> tauri::Result<()> {
    let proxy = sinkhole_port.map(sinkhole_proxy_uri);
    let ignore_hosts = proxy_ignore_hosts(is_dev);
    window.with_webview(move |webview| {
        done(webkitgtk::seal(
            &webview.inner(),
            proxy.as_deref(),
            ignore_hosts,
        ));
    })
}

/// See the Linux version. WKWebView has no sinkhole: its switches are preference features and the
/// content rule list, which WebKit compiles asynchronously, so `done` runs once that has finished.
#[cfg(target_os = "macos")]
pub fn apply_switches(
    window: &WebviewWindow,
    _sinkhole_port: Option<u16>,
    is_dev: bool,
    done: impl FnOnce(Vec<SwitchOutcome>) + Send + 'static,
) -> tauri::Result<()> {
    window.with_webview(move |webview| {
        // SAFETY: `inner()` is this window's live WKWebView, and `with_webview` runs the closure
        // on the main thread.
        unsafe { wkwebview::seal(webview.inner(), is_dev, Box::new(done)) };
    })
}

/// See the Linux version. WebView2 has nothing to switch once the window is built: its browser args
/// carried the proxy or didn't, so `done` runs at once.
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub fn apply_switches(
    _window: &WebviewWindow,
    sinkhole_port: Option<u16>,
    _is_dev: bool,
    done: impl FnOnce(Vec<SwitchOutcome>) + Send + 'static,
) -> tauri::Result<()> {
    done(vec![proxy_args_outcome(sinkhole_port)]);
    Ok(())
}

// ---- The engine switches -----------------------------------------------------------------------

/// Whether WebKitGTK `(major, minor)` still honours `enable-dns-prefetching`: it was deprecated in
/// 2.48, after which setting it does nothing and reading it always says off.
#[cfg_attr(not(any(target_os = "linux", test)), allow(dead_code))]
pub fn dns_prefetch_setting_works((major, minor): (u32, u32)) -> bool {
    (major, minor) < (2, 48)
}

/// WebKitGTK. Called on the main thread, from `with_webview`.
#[cfg(target_os = "linux")]
pub mod webkitgtk {
    use super::{dns_prefetch_setting_works, SwitchOutcome, LINUX_PROXY, LINUX_WEBRTC_OFF};
    use webkit2gtk::{
        NetworkProxyMode, NetworkProxySettings, SettingsExt, WebViewExt, WebsiteDataManagerExt,
    };

    /// Turn off WebRTC in this webview's settings, reading it back, and point its website data
    /// manager's proxy at `proxy_uri`. The data manager belongs to the web context, which every PM
    /// window shares (one data directory), so the proxy covers them all; setting it again from
    /// another window changes nothing.
    ///
    /// DNS prefetching is switched off too where the setting still works, as defence in depth, but
    /// it isn't a switch of the seal: WebKitGTK 2.48 deprecated it, and since then the setter does
    /// nothing and the getter always says off (with a warning on stderr), so a read-back could never
    /// fail. The spike measured preconnect leaking with it off anyway; the proxy is what stops that.
    /// A mod's documents also carry `X-DNS-Prefetch-Control: off` (headers.rs), but the WebKit
    /// change behind the deprecation stopped reading it (see [`super::MACOS_DNS_PREFETCHING`]).
    pub fn seal(
        webview: &webkit2gtk::WebView,
        proxy_uri: Option<&str>,
        ignore_hosts: &[&str],
    ) -> Vec<SwitchOutcome> {
        let mut out = Vec::with_capacity(2);
        match WebViewExt::settings(webview) {
            Some(settings) => {
                settings.set_enable_webrtc(false);
                out.push(if settings.enables_webrtc() {
                    SwitchOutcome::failed(LINUX_WEBRTC_OFF, "still on after switching it off")
                } else {
                    SwitchOutcome::took(LINUX_WEBRTC_OFF)
                });
                // SAFETY: plain getters of the loaded library's version numbers.
                let version = unsafe {
                    (
                        webkit2gtk::ffi::webkit_get_major_version(),
                        webkit2gtk::ffi::webkit_get_minor_version(),
                    )
                };
                if dns_prefetch_setting_works(version) {
                    settings.set_enable_dns_prefetching(false);
                }
            }
            None => {
                out.push(SwitchOutcome::failed(
                    LINUX_WEBRTC_OFF,
                    "the webview has no settings",
                ));
            }
        }
        // WebKitGTK has no getter for the proxy, so "took" means it was set.
        out.push(match (proxy_uri, webview.website_data_manager()) {
            (None, _) => SwitchOutcome::failed(LINUX_PROXY, "there is no sinkhole to point it at"),
            (Some(_), None) => {
                SwitchOutcome::failed(LINUX_PROXY, "the webview has no website data manager")
            }
            (Some(uri), Some(manager)) => {
                let mut proxy = NetworkProxySettings::new(Some(uri), ignore_hosts);
                manager.set_network_proxy_settings(NetworkProxyMode::Custom, Some(&mut proxy));
                SwitchOutcome::took(LINUX_PROXY)
            }
        });
        out
    }
}

/// WKWebView. Every call is SPI, so each is made only after `respondsToSelector:` says the selector
/// exists, and a switch whose selector is missing is reported as not taken. Read back where WebKit has
/// a getter.
#[cfg(target_os = "macos")]
pub mod wkwebview {
    use super::{
        content_rules_json, macos_feature_outcomes, names_dns_or_prefetch, DnsPrefetchSpi,
        FeatureRead, ListedFeatures, SwitchOutcome, CONTENT_RULE_LIST_ID, MACOS_FEATURES,
        MACOS_LINK_DNS_PREFETCH, MACOS_NETWORK_RULES, MACOS_PEER_CONNECTION_OFF,
    };
    use block2::RcBlock;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject, Bool, Sel};
    use objc2::{class, msg_send, sel};
    use std::cell::RefCell;
    use std::ffi::{c_char, c_void, CStr, CString};

    /// Where the outcomes go once every switch has reported.
    pub type Done = Box<dyn FnOnce(Vec<SwitchOutcome>) + Send>;

    /// Switch off WebRTC's peer connections and the four preconnect and prefetch features in this
    /// webview's preferences, then add the content rule list that blocks its network. `done` gets
    /// every outcome once the rule list has compiled (or failed to); if WebKit never calls back, it
    /// never runs, and the engine stays unsealed.
    ///
    /// # Safety
    ///
    /// `webview` must be the live `WKWebView` wry's `PlatformWebview::inner()` returns, and this must
    /// run on the main thread (inside `with_webview`).
    pub unsafe fn seal(webview: *mut c_void, is_dev: bool, done: Done) {
        let wv = webview as *mut AnyObject;
        let config: *mut AnyObject = if wv.is_null() {
            std::ptr::null_mut()
        } else {
            unsafe { msg_send![wv, configuration] }
        };
        let prefs: *mut AnyObject = if config.is_null() {
            std::ptr::null_mut()
        } else {
            unsafe { msg_send![config, preferences] }
        };
        let mut out = if prefs.is_null() {
            let why = "the webview has no preferences";
            let mut out = vec![SwitchOutcome::failed(MACOS_PEER_CONNECTION_OFF, why)];
            out.extend(
                MACOS_FEATURES
                    .iter()
                    .map(|&f| SwitchOutcome::failed(f, why)),
            );
            out
        } else {
            let mut out = vec![unsafe { peer_connection_off(prefs) }];
            out.extend(unsafe { features_off(prefs) });
            out
        };
        let controller: *mut AnyObject = if config.is_null() {
            std::ptr::null_mut()
        } else {
            unsafe { msg_send![config, userContentController] }
        };
        // SAFETY: `controller` is null or the live controller `configuration` handed back; retaining
        // it keeps it alive until WebKit calls back.
        match unsafe { Retained::retain(controller) } {
            Some(controller) => unsafe { network_rules(controller, is_dev, out, done) },
            None => {
                out.push(SwitchOutcome::failed(
                    MACOS_NETWORK_RULES,
                    "the webview has no user content controller",
                ));
                done(out);
            }
        }
    }

    /// Compile [`content_rules_json`] and add it to `controller`, then hand `done` the outcomes with
    /// this one's added. Public API since macOS 10.13, checked with `respondsToSelector:` all the
    /// same, so a missing piece is a switch that didn't take, never a crash.
    unsafe fn network_rules(
        controller: Retained<AnyObject>,
        is_dev: bool,
        out: Vec<SwitchOutcome>,
        done: Done,
    ) {
        let failed = |mut out: Vec<SwitchOutcome>, done: Done, why: &str| {
            out.push(SwitchOutcome::failed(MACOS_NETWORK_RULES, why));
            done(out);
        };
        let Some(store_class) = AnyClass::get(c"WKContentRuleListStore") else {
            return failed(out, done, "WebKit has no WKContentRuleListStore");
        };
        let store: *mut AnyObject = unsafe { msg_send![store_class, defaultStore] };
        if store.is_null() {
            return failed(out, done, "WebKit has no default content rule list store");
        }
        let compile =
            sel!(compileContentRuleListForIdentifier:encodedContentRuleList:completionHandler:);
        if !unsafe { responds(store, compile) } {
            return failed(out, done, "WebKit can't compile a content rule list");
        }
        let can_add = unsafe {
            responds(
                Retained::as_ptr(&controller).cast_mut(),
                sel!(addContentRuleList:),
            )
        };
        if !can_add {
            return failed(out, done, "WebKit can't add a content rule list");
        }
        let (Ok(id), Ok(json)) = (
            CString::new(CONTENT_RULE_LIST_ID),
            CString::new(content_rules_json(is_dev)),
        ) else {
            return failed(out, done, "the rule list isn't a C string");
        };
        let id = unsafe { ns_string(&id) };
        let json = unsafe { ns_string(&json) };
        if id.is_null() || json.is_null() {
            return failed(out, done, "couldn't make the rule list's strings");
        }
        // The block is `Fn`, and WebKit calls it once: the outcomes and `done` are taken on the
        // first call, and a second call (which WebKit never makes) finds nothing.
        let pending = RefCell::new(Some((out, done)));
        let block = RcBlock::new(move |list: *mut AnyObject, error: *mut AnyObject| {
            let Some((mut out, done)) = pending.borrow_mut().take() else {
                return;
            };
            // SAFETY: plain class method.
            let on_main: Bool = unsafe { msg_send![class!(NSThread), isMainThread] };
            out.push(if list.is_null() {
                SwitchOutcome::failed(
                    MACOS_NETWORK_RULES,
                    format!("WebKit didn't compile it: {}", unsafe { describe(error) }),
                )
            } else if !on_main.as_bool() {
                // A user content controller is main-thread-only; refuse rather than touch it.
                SwitchOutcome::failed(
                    MACOS_NETWORK_RULES,
                    "WebKit called back off the main thread",
                )
            } else {
                // SAFETY: `controller` is retained and responds to `addContentRuleList:`, and `list`
                // is the live list WebKit is handing over for the length of this call (the
                // controller retains it).
                let _: () = unsafe { msg_send![&*controller, addContentRuleList: list] };
                SwitchOutcome::took(MACOS_NETWORK_RULES)
            });
            done(out);
        });
        // SAFETY: `store` responds to the selector; both strings are live NSStrings; WebKit copies
        // the block before returning.
        let _: () = unsafe {
            msg_send![
                store,
                compileContentRuleListForIdentifier: id,
                encodedContentRuleList: json,
                completionHandler: &*block
            ]
        };
    }

    /// An autoreleased NSString with `s`'s text, or null.
    unsafe fn ns_string(s: &CStr) -> *mut AnyObject {
        unsafe { msg_send![class!(NSString), stringWithUTF8String: s.as_ptr()] }
    }

    /// An NSError's `localizedDescription`, for the Developer tab's reason.
    unsafe fn describe(error: *mut AnyObject) -> String {
        if error.is_null() {
            return "no error given".to_owned();
        }
        let text: *mut AnyObject = unsafe { msg_send![error, localizedDescription] };
        if text.is_null() {
            return "an error with no description".to_owned();
        }
        let utf8: *const c_char = unsafe { msg_send![text, UTF8String] };
        if utf8.is_null() {
            return "an error with no description".to_owned();
        }
        unsafe { CStr::from_ptr(utf8) }
            .to_string_lossy()
            .into_owned()
    }

    unsafe fn responds(obj: *mut AnyObject, selector: Sel) -> bool {
        let yes: Bool = unsafe { msg_send![obj, respondsToSelector: selector] };
        yes.as_bool()
    }

    unsafe fn peer_connection_off(prefs: *mut AnyObject) -> SwitchOutcome {
        unsafe {
            if !responds(prefs, sel!(_setPeerConnectionEnabled:)) {
                return SwitchOutcome::failed(
                    MACOS_PEER_CONNECTION_OFF,
                    "WebKit has no -[WKPreferences _setPeerConnectionEnabled:]",
                );
            }
            let _: () = msg_send![prefs, _setPeerConnectionEnabled: Bool::NO];
            if responds(prefs, sel!(_peerConnectionEnabled)) {
                let on: Bool = msg_send![prefs, _peerConnectionEnabled];
                if on.as_bool() {
                    return SwitchOutcome::failed(
                        MACOS_PEER_CONNECTION_OFF,
                        "still on after switching it off",
                    );
                }
            }
        }
        SwitchOutcome::took(MACOS_PEER_CONNECTION_OFF)
    }

    /// The four features, by the `_features` route, with DNS prefetching's own setter where that
    /// route finds neither of its keys (see [`super::MACOS_DNS_PREFETCHING`]).
    unsafe fn features_off(prefs: *mut AnyObject) -> Vec<SwitchOutcome> {
        let listed = unsafe { listed_features_off(prefs) };
        macos_feature_outcomes(listed, || unsafe { dns_prefetching_off(prefs) })
    }

    /// Switch off each of [`MACOS_FEATURES`] and [`MACOS_LINK_DNS_PREFETCH`] that
    /// `+[WKPreferences _features]` lists, reading it back where WebKit can, and note every listed
    /// key that mentions DNS or prefetching. `Err` when there is no list to go by.
    unsafe fn listed_features_off(prefs: *mut AnyObject) -> Result<ListedFeatures, &'static str> {
        let mut listed = ListedFeatures::default();
        unsafe {
            let has_list: Bool =
                msg_send![class!(WKPreferences), respondsToSelector: sel!(_features)];
            if !has_list.as_bool() {
                return Err("WebKit has no +[WKPreferences _features]");
            }
            if !responds(prefs, sel!(_setEnabled:forFeature:)) {
                return Err("WebKit has no -[WKPreferences _setEnabled:forFeature:]");
            }
            let can_read = responds(prefs, sel!(_isEnabledForFeature:));
            let features: *mut AnyObject = msg_send![class!(WKPreferences), _features];
            if features.is_null() {
                return Err("WebKit listed no features");
            }
            let count: usize = msg_send![features, count];
            for i in 0..count {
                let feature: *mut AnyObject = msg_send![features, objectAtIndex: i];
                if feature.is_null() {
                    continue;
                }
                let key: *mut AnyObject = msg_send![feature, key];
                if key.is_null() {
                    continue;
                }
                let utf8: *const c_char = msg_send![key, UTF8String];
                if utf8.is_null() {
                    continue;
                }
                let name = CStr::from_ptr(utf8).to_string_lossy();
                if names_dns_or_prefetch(&name) {
                    listed.dns_named.push(name.as_ref().to_owned());
                }
                let slot = MACOS_FEATURES.iter().position(|f| *f == name.as_ref());
                let link_dns = MACOS_LINK_DNS_PREFETCH == name.as_ref();
                if slot.is_none() && !link_dns {
                    continue;
                }
                let _: () = msg_send![prefs, _setEnabled: Bool::NO, forFeature: feature];
                let read = if can_read {
                    let on: Bool = msg_send![prefs, _isEnabledForFeature: feature];
                    if on.as_bool() {
                        FeatureRead::StillOn
                    } else {
                        FeatureRead::Off
                    }
                } else {
                    FeatureRead::NoGetter
                };
                match slot {
                    Some(slot) => {
                        listed.outcomes[slot] = Some(if read == FeatureRead::StillOn {
                            SwitchOutcome::failed(
                                MACOS_FEATURES[slot],
                                "still on after switching it off",
                            )
                        } else {
                            SwitchOutcome::took(MACOS_FEATURES[slot])
                        });
                    }
                    None => listed.link_dns = Some(read),
                }
            }
        }
        Ok(listed)
    }

    /// DNS prefetching through `-[WKPreferences _setDNSPrefetchingEnabled:]`, for a WebKit whose
    /// `_features` lists neither of its keys. Switched on and read back, then off and read back, so
    /// a working preference is told from the stubs of a removed one (see [`DnsPrefetchSpi`]);
    /// without a getter it is still switched off, but doesn't count.
    unsafe fn dns_prefetching_off(prefs: *mut AnyObject) -> DnsPrefetchSpi {
        unsafe {
            if !responds(prefs, sel!(_setDNSPrefetchingEnabled:)) {
                return DnsPrefetchSpi::NoSetter;
            }
            if !responds(prefs, sel!(_dnsPrefetchingEnabled)) {
                let _: () = msg_send![prefs, _setDNSPrefetchingEnabled: Bool::NO];
                return DnsPrefetchSpi::NoGetter;
            }
            let _: () = msg_send![prefs, _setDNSPrefetchingEnabled: Bool::YES];
            let followed_on: Bool = msg_send![prefs, _dnsPrefetchingEnabled];
            let _: () = msg_send![prefs, _setDNSPrefetchingEnabled: Bool::NO];
            let still_on: Bool = msg_send![prefs, _dnsPrefetchingEnabled];
            if still_on.as_bool() {
                DnsPrefetchSpi::StillOn
            } else if !followed_on.as_bool() {
                DnsPrefetchSpi::Stubbed
            } else {
                DnsPrefetchSpi::Off
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::io::Read;
    use std::net::TcpStream;
    use std::time::Instant;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    fn never(_: &Url) -> bool {
        false
    }
    fn always(_: &Url) -> bool {
        true
    }
    const NEVER: &dyn Fn(&Url) -> bool = &never;
    const ALWAYS: &dyn Fn(&Url) -> bool = &always;

    fn allows(platform: Platform, s: &str, is_dev: bool, pending: &dyn Fn(&Url) -> bool) -> bool {
        allow_navigation_on(platform, &url(s), is_dev, pending)
    }

    #[test]
    fn the_guard_lets_pm_load_its_own_pages_and_nothing_else_from_the_web() {
        for platform in [Platform::Linux, Platform::MacOs] {
            for s in [
                "tauri://localhost",
                "tauri://localhost/",
                "tauri://localhost/index.html?window=calendar",
                "tauri://localhost/index.html?window=briefing#x",
            ] {
                assert!(allows(platform, s, false, NEVER), "{platform:?} {s}");
            }
            for s in [
                // WebView2's name for PM's scheme is an ordinary http URL on WebKit.
                "http://tauri.localhost/",
                "https://tauri.localhost/",
                "tauri://localhost:8080/",
                "tauri://localhost.example.com/",
                "https://example.com/",
                "http://localhost:1420/",
                "file:///etc/passwd",
                "data:text/html,<p>hi</p>",
                "blob:tauri://localhost/0a1b",
                "javascript:alert(1)",
                "mailto:someone@example.com",
                "about:srcdoc",
                "about:config",
            ] {
                assert!(!allows(platform, s, false, ALWAYS), "{platform:?} {s}");
            }
        }
        assert!(allows(
            Platform::Windows,
            "http://tauri.localhost/",
            false,
            NEVER
        ));
        assert!(allows(
            Platform::Windows,
            "http://tauri.localhost/index.html?window=calendar",
            false,
            NEVER
        ));
        for s in [
            "tauri://localhost/",
            // `useHttpsScheme` is off, so this would be a real network URL.
            "https://tauri.localhost/",
            "http://tauri.localhost:8080/",
            "https://example.com/",
            "about:srcdoc",
        ] {
            assert!(!allows(Platform::Windows, s, false, ALWAYS), "{s}");
        }
    }

    #[test]
    fn about_blank_is_allowed_everywhere_since_a_blank_frame_navigates_to_it() {
        for platform in [Platform::Linux, Platform::MacOs, Platform::Windows] {
            assert!(allows(platform, "about:blank", false, NEVER));
            assert!(allows(platform, "about:blank#top", true, NEVER));
        }
    }

    #[test]
    fn the_dev_server_is_allowed_only_in_a_dev_build() {
        for platform in [Platform::Linux, Platform::MacOs, Platform::Windows] {
            for s in [
                "http://localhost:1420/",
                "http://localhost:1420/index.html?window=calendar",
                "http://127.0.0.1:1420/",
            ] {
                assert!(allows(platform, s, true, NEVER), "{platform:?} {s}");
                assert!(!allows(platform, s, false, NEVER), "{platform:?} {s}");
            }
            for s in [
                "https://localhost:1420/",
                "http://localhost.example.com:1420/",
                "http://192.168.1.2:1420/",
                "ws://localhost:1421/",
            ] {
                assert!(!allows(platform, s, true, ALWAYS), "{platform:?} {s}");
            }
        }
    }

    #[test]
    fn a_mod_url_passes_only_as_a_pending_entry_and_only_where_the_guard_sees_frames() {
        let entry = "pmmod://localhost/atlas.map/index.html?entry=abc";
        for platform in [Platform::Linux, Platform::MacOs] {
            assert!(allows(platform, entry, false, ALWAYS));
            assert!(!allows(platform, entry, false, NEVER));
            assert!(!allows(platform, entry, true, NEVER));
            // Another host on the scheme is not a mod's URL, whatever the entry table says.
            assert!(!allows(platform, "pmmod://evil/atlas.map/", false, ALWAYS));
        }
        // WebView2 asks only about the top level, where a mod's page must never land.
        let entry = "http://pmmod.localhost/atlas.map/index.html?entry=abc";
        assert!(!allows(Platform::Windows, entry, false, ALWAYS));
        assert!(!allows(Platform::Windows, entry, true, ALWAYS));
    }

    #[test]
    fn the_entry_table_is_asked_only_about_mod_urls() {
        let asked = RefCell::new(Vec::new());
        let pending = |u: &Url| {
            asked.borrow_mut().push(u.to_string());
            true
        };
        for s in [
            "tauri://localhost/",
            "https://example.com/",
            "about:blank",
            "http://localhost:1420/",
            "pmmod://localhost/atlas.map/index.html",
        ] {
            allows(Platform::Linux, s, true, &pending);
        }
        assert_eq!(
            *asked.borrow(),
            vec!["pmmod://localhost/atlas.map/index.html".to_string()]
        );
    }

    #[test]
    fn the_browser_args_keep_wrys_defaults_and_add_the_sinkhole() {
        assert_eq!(
            WRY_DEFAULT_BROWSER_ARGS,
            "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection"
        );
        assert_eq!(
            windows_browser_args(Some(4321), false),
            "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection \
             --proxy-server=http://127.0.0.1:4321 --proxy-bypass-list=<-loopback>"
        );
    }

    #[test]
    fn a_dev_builds_args_let_loopback_through_for_vite() {
        let args = windows_browser_args(Some(4321), true);
        assert_eq!(
            args,
            "--disable-features=msWebOOUI,msPdfOOUI,msSmartScreenProtection \
             --proxy-server=http://127.0.0.1:4321"
        );
        assert!(!args.contains("<-loopback>"));
    }

    #[test]
    fn with_no_sinkhole_the_args_are_wrys_and_the_switch_reports_failed() {
        assert_eq!(windows_browser_args(None, false), WRY_DEFAULT_BROWSER_ARGS);
        assert_eq!(
            proxy_args_outcome(Some(1)),
            SwitchOutcome::took(WINDOWS_PROXY_ARGS)
        );
        assert!(!proxy_args_outcome(None).took);
    }

    #[test]
    fn the_linux_proxy_ignores_only_vite_and_only_in_dev() {
        assert!(proxy_ignore_hosts(false).is_empty());
        assert_eq!(proxy_ignore_hosts(true), ["localhost", "127.0.0.1"]);
        assert_eq!(sinkhole_proxy_uri(4321), "http://127.0.0.1:4321");
    }

    fn all_took(platform: Platform) -> Vec<SwitchOutcome> {
        platform
            .expected_switches()
            .iter()
            .copied()
            .map(SwitchOutcome::took)
            .collect()
    }

    #[test]
    fn every_switch_taking_seals_the_engine() {
        for platform in [Platform::Linux, Platform::MacOs, Platform::Windows] {
            let v = verdict(platform.expected_switches(), &all_took(platform));
            assert!(v.sealed, "{platform:?}: {:?}", v.reasons);
            assert!(v.reasons.is_empty());
        }
    }

    #[test]
    fn one_switch_that_didnt_take_unseals_it_and_says_which_and_why() {
        let mut outcomes = all_took(Platform::Linux);
        outcomes[1] = SwitchOutcome::failed(LINUX_PROXY, "the webview has no website data manager");
        let v = verdict(Platform::Linux.expected_switches(), &outcomes);
        assert!(!v.sealed);
        assert_eq!(
            v.reasons,
            ["webkitgtk.proxy didn't take: the webview has no website data manager"]
        );
    }

    #[test]
    fn a_switch_that_never_reported_unseals_it() {
        let outcomes: Vec<_> = all_took(Platform::MacOs).into_iter().skip(1).collect();
        let v = verdict(Platform::MacOs.expected_switches(), &outcomes);
        assert!(!v.sealed);
        assert_eq!(
            v.reasons,
            ["wkwebview.peer-connection-off was never applied"]
        );
        // Nothing reported at all: before the main window exists.
        let v = verdict(Platform::Windows.expected_switches(), &[]);
        assert!(!v.sealed);
        assert_eq!(v.reasons.len(), 2);
    }

    #[test]
    fn the_verdict_is_final_only_once_every_expected_switch_has_reported() {
        for platform in [Platform::Linux, Platform::MacOs, Platform::Windows] {
            let expected = platform.expected_switches();
            assert_eq!(unreported(expected, &[]), expected);
            let mut outcomes = all_took(platform);
            // The last to report (macOS: the rule list, from WebKit's completion handler).
            let last = outcomes.pop().unwrap();
            assert_eq!(unreported(expected, &outcomes), [last.name], "{platform:?}");
            // A failed report is a report: the verdict is final, and unsealed.
            outcomes.push(SwitchOutcome::failed(last.name, "no"));
            assert!(unreported(expected, &outcomes).is_empty(), "{platform:?}");
            // A switch nobody expected reports nothing that was expected.
            assert_eq!(
                unreported(expected, &[SwitchOutcome::took("other")]),
                expected
            );
        }
    }

    #[test]
    fn a_window_is_built_hidden_and_shown_only_if_its_config_says_visible() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
        let main: WindowConfig = serde_json::from_value(conf["app"]["windows"][0].clone()).unwrap();
        assert!(main.visible && main.maximized);
        let (built, show) = hidden_until_built(&main);
        assert!(!built.visible);
        assert!(show);
        // Nothing else changes: still maximized, still frameless, still PM's main window.
        assert_eq!(
            WindowConfig {
                visible: true,
                ..built
            },
            main
        );
        let hidden = WindowConfig {
            visible: false,
            ..main
        };
        assert_eq!(hidden_until_built(&hidden), (hidden.clone(), false));
    }

    #[test]
    fn a_later_failure_of_a_switch_that_took_still_unseals_it() {
        let mut outcomes = all_took(Platform::Windows);
        outcomes.push(SwitchOutcome::failed(SINKHOLE, "gone"));
        assert!(!verdict(Platform::Windows.expected_switches(), &outcomes).sealed);
    }

    #[test]
    fn an_unexpected_failure_unseals_it_and_an_unexpected_success_changes_nothing() {
        let mut outcomes = all_took(Platform::Windows);
        outcomes.push(SwitchOutcome::took(LINUX_PROXY));
        assert!(verdict(Platform::Windows.expected_switches(), &outcomes).sealed);
        outcomes.push(SwitchOutcome::failed(LINUX_PROXY, "no manager"));
        let v = verdict(Platform::Windows.expected_switches(), &outcomes);
        assert!(!v.sealed);
        assert_eq!(v.reasons, ["webkitgtk.proxy didn't take: no manager"]);
    }

    #[test]
    fn each_engine_expects_the_switches_the_spike_measured() {
        // WebKitGTK's DNS-prefetching setting isn't one: since 2.48 its read-back can't fail.
        assert_eq!(
            Platform::Linux.expected_switches(),
            [SINKHOLE, LINUX_PROXY, LINUX_WEBRTC_OFF]
        );
        assert_eq!(
            Platform::Windows.expected_switches(),
            [SINKHOLE, WINDOWS_PROXY_ARGS]
        );
        assert_eq!(
            Platform::MacOs.expected_switches(),
            [
                MACOS_PEER_CONNECTION_OFF,
                "LinkPreconnect",
                "LinkPreconnectEarlyHintsEnabled",
                "LinkPrefetchEnabled",
                "DNSPrefetchingEnabled",
                MACOS_NETWORK_RULES,
            ]
        );
        // The proxy didn't take on macOS in any form, so macOS has no sinkhole to depend on: its
        // network wall is the content rule list.
        assert!(!Platform::MacOs.uses_sinkhole());
        assert!(Platform::Linux.uses_sinkhole() && Platform::Windows.uses_sinkhole());
    }

    #[test]
    fn the_dns_prefetch_setting_is_trusted_only_before_webkitgtk_2_48() {
        for old in [(2, 40), (2, 46), (2, 47), (1, 99)] {
            assert!(dns_prefetch_setting_works(old), "{old:?}");
        }
        for new in [(2, 48), (2, 54), (3, 0)] {
            assert!(!dns_prefetch_setting_works(new), "{new:?}");
        }
    }

    /// What the `_features` route found on macos-14 (all four) and on macos-latest (no DNS).
    fn listed(dns: bool, dns_named: &[&str]) -> ListedFeatures {
        let mut l = ListedFeatures {
            dns_named: dns_named.iter().map(|k| (*k).to_owned()).collect(),
            ..ListedFeatures::default()
        };
        for (slot, &name) in MACOS_FEATURES.iter().enumerate() {
            if dns || name != MACOS_DNS_PREFETCHING {
                l.outcomes[slot] = Some(SwitchOutcome::took(name));
            }
        }
        l
    }

    #[test]
    fn dns_prefetching_falls_back_to_its_own_setter_only_where_it_isnt_listed() {
        let names = |out: &[SwitchOutcome]| out.iter().map(|o| o.name).collect::<Vec<_>>();
        // Listed (macos-14): the setter isn't tried.
        let out = macos_feature_outcomes(Ok(listed(true, &[])), || {
            panic!("the setter was tried for a listed feature")
        });
        assert_eq!(names(&out), MACOS_FEATURES);
        assert!(out.iter().all(|o| o.took));
        // Neither key listed (WebKit 7625 on): the setter decides. A getter that followed it is a
        // preference switched off; one that read off both times is the removed preference's stubs,
        // gone, with the reason and the listed keys in the detail; anything else didn't take.
        let gone = format!("{DNS_PREFETCHING_GONE}; no listed feature mentions DNS or prefetch");
        for (spi, took, detail) in [
            (DnsPrefetchSpi::Off, true, Some("")),
            (DnsPrefetchSpi::Stubbed, true, Some(gone.as_str())),
            (DnsPrefetchSpi::NoSetter, false, None),
            (DnsPrefetchSpi::NoGetter, false, None),
            (DnsPrefetchSpi::StillOn, false, None),
        ] {
            let mut tried = 0;
            let out = macos_feature_outcomes(Ok(listed(false, &[])), || {
                tried += 1;
                spi
            });
            assert_eq!(tried, 1, "{spi:?}");
            assert_eq!(names(&out), MACOS_FEATURES, "{spi:?}");
            assert_eq!(out[3].took, took, "{spi:?}: {}", out[3].detail);
            match detail {
                Some(detail) => assert_eq!(out[3].detail, detail, "{spi:?}"),
                None => assert!(!out[3].detail.is_empty(), "{spi:?}"),
            }
            assert!(out[..3].iter().all(|o| o.took), "{spi:?}");
        }
        // No list to go by: the other three fail with the reason, DNS still gets its setter.
        let out = macos_feature_outcomes(Err("WebKit has no +[WKPreferences _features]"), || {
            DnsPrefetchSpi::Off
        });
        assert_eq!(names(&out), MACOS_FEATURES);
        for o in &out[..3] {
            assert!(!o.took);
            assert_eq!(o.detail, "WebKit has no +[WKPreferences _features]");
        }
        assert!(out[3].took);
        let out = macos_feature_outcomes(Err("WebKit listed no features"), || {
            DnsPrefetchSpi::NoSetter
        });
        assert_eq!(
            out[3].detail,
            "WebKit listed no features; WebKit has no -[WKPreferences _setDNSPrefetchingEnabled:]"
        );
        // With no list, the stubs aren't enough: LinkDNSPrefetchEnabled may be there unswitched.
        let out =
            macos_feature_outcomes(Err("WebKit listed no features"), || DnsPrefetchSpi::Stubbed);
        assert_eq!(
            out[3],
            SwitchOutcome::failed(
                MACOS_DNS_PREFETCHING,
                "WebKit listed no features; -[WKPreferences _setDNSPrefetchingEnabled:] does \
                 nothing and _dnsPrefetchingEnabled always reads off, but with no list of \
                 features to go by, LinkDNSPrefetchEnabled may still be there to switch off"
            )
        );
        // The engine stays unsealed unless the switch took.
        let mut all = vec![SwitchOutcome::took(MACOS_PEER_CONNECTION_OFF)];
        all.extend(macos_feature_outcomes(Ok(listed(false, &[])), || {
            DnsPrefetchSpi::NoSetter
        }));
        all.push(SwitchOutcome::took(MACOS_NETWORK_RULES));
        assert!(!verdict(Platform::MacOs.expected_switches(), &all).sealed);
    }

    /// Where the preference is gone, DNS prefetching is left to the content rule list: sealed with
    /// it, unsealed without it. The stubs count as gone only where WebKit listed its features, and
    /// the detail keeps the listed keys.
    #[test]
    fn a_removed_dns_prefetching_preference_leaves_the_seal_to_the_rule_list() {
        let macos = |dns: DnsPrefetchSpi, rules: SwitchOutcome| {
            let mut all = vec![SwitchOutcome::took(MACOS_PEER_CONNECTION_OFF)];
            all.extend(macos_feature_outcomes(Ok(listed(false, &[])), || dns));
            all.push(rules);
            verdict(Platform::MacOs.expected_switches(), &all)
        };
        let v = macos(
            DnsPrefetchSpi::Stubbed,
            SwitchOutcome::took(MACOS_NETWORK_RULES),
        );
        assert!(v.sealed, "{:?}", v.reasons);
        let v = macos(
            DnsPrefetchSpi::Stubbed,
            SwitchOutcome::failed(MACOS_NETWORK_RULES, "WebKit didn't compile it"),
        );
        assert_eq!(
            v.reasons,
            ["wkwebview.content-rule-list didn't take: WebKit didn't compile it"]
        );
        for (listed, tail) in [
            (&[][..], "; no listed feature mentions DNS or prefetch"),
            (
                &["LinkPrefetchEnabled".to_owned()][..],
                "; the listed features that mention DNS or prefetch: LinkPrefetchEnabled",
            ),
        ] {
            assert_eq!(
                dns_prefetch_outcome("x", DnsPrefetchSpi::Stubbed, Some(listed)),
                SwitchOutcome::gone(
                    MACOS_DNS_PREFETCHING,
                    format!("{DNS_PREFETCHING_GONE}{tail}")
                )
            );
        }
        assert!(!dns_prefetch_outcome("x", DnsPrefetchSpi::Stubbed, None).took);
    }

    /// WebKit 7622 to 7624: `DNSPrefetchingEnabled` is gone and `<link rel=dns-prefetch>` has a
    /// preference of its own, switched off through `_features` and counted only if it reads back
    /// off. The setter isn't tried, and the stubs can't stand in for it.
    #[test]
    fn link_dns_prefetch_is_switched_off_where_webkit_lists_it() {
        let with_link = |read: FeatureRead| {
            let mut l = listed(false, &["LinkDNSPrefetchEnabled", "LinkPrefetchEnabled"]);
            l.link_dns = Some(read);
            l
        };
        let never = || -> DnsPrefetchSpi { panic!("the setter was tried with a key listed") };
        let out = macos_feature_outcomes(Ok(with_link(FeatureRead::Off)), never);
        assert_eq!(
            out[3],
            SwitchOutcome::took_as(MACOS_DNS_PREFETCHING, LINK_DNS_PREFETCH_OFF)
        );
        for (read, detail) in [
            (
                FeatureRead::StillOn,
                "LinkDNSPrefetchEnabled still on after switching it off",
            ),
            (
                FeatureRead::NoGetter,
                "LinkDNSPrefetchEnabled switched off, but WebKit has no -[WKPreferences \
                 _isEnabledForFeature:] to read it back",
            ),
        ] {
            let out = macos_feature_outcomes(Ok(with_link(read)), never);
            assert_eq!(
                out[3],
                SwitchOutcome::failed(MACOS_DNS_PREFETCHING, detail),
                "{read:?}"
            );
        }
        // No tag lists both; if one did, both are switched off and both must take.
        let both = |dns: SwitchOutcome, read: FeatureRead| {
            let mut l = with_link(read);
            l.outcomes[3] = Some(dns);
            macos_feature_outcomes(Ok(l), never).remove(3)
        };
        let dns_failed = SwitchOutcome::failed(MACOS_DNS_PREFETCHING, "still on");
        assert_eq!(both(dns_failed.clone(), FeatureRead::Off), dns_failed);
        let dns_took = SwitchOutcome::took(MACOS_DNS_PREFETCHING);
        assert!(!both(dns_took.clone(), FeatureRead::StillOn).took);
        assert!(both(dns_took, FeatureRead::Off).took);
        // The seal: taken with the rule list, unsealed when it reads on.
        let macos = |read: FeatureRead| {
            let mut all = vec![SwitchOutcome::took(MACOS_PEER_CONNECTION_OFF)];
            all.extend(macos_feature_outcomes(Ok(with_link(read)), never));
            all.push(SwitchOutcome::took(MACOS_NETWORK_RULES));
            verdict(Platform::MacOs.expected_switches(), &all)
        };
        assert!(macos(FeatureRead::Off).sealed);
        assert_eq!(
            macos(FeatureRead::StillOn).reasons,
            ["DNSPrefetchingEnabled didn't take: LinkDNSPrefetchEnabled still on after switching \
              it off"]
        );
    }

    #[test]
    fn a_dns_prefetching_failure_names_what_this_webkit_lists() {
        let keys = |n: usize| (0..n).map(|i| format!("DNSThing{i}")).collect::<Vec<_>>();
        let detail = |listed: Option<&[String]>| {
            dns_prefetch_outcome(
                "WebKit lists no such feature",
                DnsPrefetchSpi::NoSetter,
                listed,
            )
            .detail
        };
        assert_eq!(
            detail(Some(&[
                "LinkPrefetchEnabled".to_owned(),
                "dnsOverHTTPS".to_owned()
            ])),
            "WebKit lists no such feature; WebKit has no -[WKPreferences \
             _setDNSPrefetchingEnabled:]; the listed features that mention DNS or prefetch: \
             LinkPrefetchEnabled, dnsOverHTTPS"
        );
        assert!(detail(Some(&[])).ends_with("; no listed feature mentions DNS or prefetch"));
        assert!(detail(Some(&keys(16))).ends_with("DNSThing14, DNSThing15"));
        assert!(detail(Some(&keys(20))).ends_with("DNSThing15 and 4 more"));
        // Every failure carries the list, not only a missing setter.
        for spi in [DnsPrefetchSpi::NoGetter, DnsPrefetchSpi::StillOn] {
            let o = dns_prefetch_outcome("x", spi, Some(&keys(1)));
            assert!(!o.took);
            assert!(o.detail.ends_with(": DNSThing0"), "{spi:?}: {}", o.detail);
        }
        for (key, named) in [
            ("DNSPrefetchingEnabled", true),
            ("LinkDNSPrefetchEnabled", true),
            ("dnsPrefetching", true),
            ("LinkPrefetchEnabled", true),
            ("SpeculativePREFETCH", true),
            ("LinkPreconnect", false),
            ("WebRTCEnabled", false),
        ] {
            assert_eq!(names_dns_or_prefetch(key), named, "{key}");
        }
    }

    fn rules(is_dev: bool) -> Vec<(String, String)> {
        let parsed: serde_json::Value = serde_json::from_str(&content_rules_json(is_dev)).unwrap();
        parsed
            .as_array()
            .unwrap()
            .iter()
            .map(|r| {
                assert_eq!(r.as_object().unwrap().len(), 2, "{r}");
                assert_eq!(r["trigger"].as_object().unwrap().len(), 1, "{r}");
                (
                    r["trigger"]["url-filter"].as_str().unwrap().to_owned(),
                    r["action"]["type"].as_str().unwrap().to_owned(),
                )
            })
            .collect()
    }

    #[test]
    fn the_macos_rule_list_blocks_every_web_scheme_and_in_dev_lets_only_vite_through() {
        let block = |f: &str| (f.to_owned(), "block".to_owned());
        assert_eq!(rules(false), [block("^https?://"), block("^wss?://")]);
        let dev = rules(true);
        assert_eq!(dev[..2], [block("^https?://"), block("^wss?://")]);
        // The exemptions come after the blocks, or they would exempt nothing.
        let ignore = |f: &str| (f.to_owned(), "ignore-previous-rules".to_owned());
        assert_eq!(
            dev[2..],
            [
                ignore("^https?://localhost(:[0-9]+)?/"),
                ignore("^https?://127\\.0\\.0\\.1(:[0-9]+)?/"),
                ignore("^wss?://localhost(:[0-9]+)?/"),
                ignore("^wss?://127\\.0\\.0\\.1(:[0-9]+)?/"),
            ]
        );
        // Each exemption names the whole authority: nothing can come between the host (or its
        // port) and the path, so userinfo naming Vite's host exempts nothing. `mods::tests` runs
        // the filters against URLs (this file's tests also build in the regression app, which has
        // no regex crate).
        for (filter, _) in &dev[2..] {
            assert!(filter.ends_with("(:[0-9]+)?/"), "{filter}");
        }
        // WebKit's rule regexes have no disjunction; a `|` would fail the compile, and with it the
        // seal. Nor is any scheme PM's own page uses ever named.
        for (filter, _) in rules(true) {
            assert!(!filter.contains('|'), "{filter}");
            for own in ["tauri", "ipc", "pmmod"] {
                assert!(!filter.contains(own), "{filter}");
            }
        }
    }

    /// The reason the guard isn't the last wall: tauri-runtime-wry allows a navigation whose URL it
    /// can't parse, and some engines accept this host (WebView2 and Fedora's WebKitGTK 2.54; the
    /// WebKit on ubuntu-latest and macos-latest refuses it as an invalid URL). If the `url` crate
    /// ever starts accepting it, the sealed-frame job's `idna-host` target no longer reaches that
    /// path and needs another host.
    #[test]
    fn rust_refuses_a_host_webkit_accepts() {
        assert!(Url::parse("http://xn--1ug.pm-probe.test:8080/x").is_err());
        assert!(Url::parse("https://xn--zz.example.invalid/").is_err());
    }

    #[test]
    fn a_remembered_port_is_one_unprivileged_number() {
        assert_eq!(parse_remembered_port("49152"), Some(49152));
        assert_eq!(parse_remembered_port(" 1024\n"), Some(1024));
        for bad in [
            "",
            " ",
            "0",
            "80",
            "1023",
            "65536",
            "-1",
            "+5000",
            "5000x",
            "50 00",
            "0x1f90",
            "５０００",
        ] {
            assert_eq!(parse_remembered_port(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn the_sinkhole_takes_its_remembered_port_when_free_and_a_fresh_one_when_not() {
        let first = Sinkhole::bind_preferring(None).unwrap();
        // Taken (by `first`): a fresh port, not an error.
        let second = Sinkhole::bind_preferring(Some(first.port())).unwrap();
        assert_ne!(second.port(), first.port());
        // Free: that port.
        let free = {
            let probe = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
            probe.local_addr().unwrap().port()
        };
        let third = Sinkhole::bind_preferring(Some(free)).unwrap();
        assert_eq!(third.port(), free);
        assert_ne!(Sinkhole::bind_preferring(None).unwrap().port(), 0);
    }

    #[test]
    fn webview2_args_count_as_overridden_by_the_variable_or_a_policy() {
        use std::ffi::OsStr;
        assert_eq!(webview2_args_override(None, None), None);
        for set in ["--no-proxy-server", " ", ""] {
            let why = webview2_args_override(Some(OsStr::new(set)), None).unwrap();
            assert!(why.contains(WEBVIEW2_ARGS_ENV), "{set:?}: {why}");
        }
        let why = webview2_args_override(None, Some("HKLM\\…\\pm.exe")).unwrap();
        assert!(why.contains("HKLM\\…\\pm.exe"), "{why}");
        assert_eq!(
            webview2_policy_value_names(Some("pm.exe"), "org.itsatlas.pm"),
            ["pm.exe", "org.itsatlas.pm", "*"]
        );
        assert_eq!(
            webview2_policy_value_names(None, "org.itsatlas.pm"),
            ["org.itsatlas.pm", "*"]
        );
        assert_eq!(webview2_policy_value_names(Some(""), ""), ["*"]);
    }

    #[test]
    fn only_windows_retries_a_window_build_and_only_until_it_works() {
        // Elsewhere: one try, its error returned.
        for platform in [Platform::Linux, Platform::MacOs] {
            let mut tries = 0;
            let mut waited = Vec::new();
            let r: Result<(), String> = build_retrying(
                platform,
                || {
                    tries += 1;
                    Err(format!("no {tries}"))
                },
                |d| waited.push(d),
            );
            assert_eq!((r, tries, waited.len()), (Err("no 1".into()), 1, 0));
        }
        // Windows: it works on the third try, after the first two waits.
        let mut tries = 0;
        let mut waited = Vec::new();
        let r: Result<u32, String> = build_retrying(
            Platform::Windows,
            || {
                tries += 1;
                if tries < 3 {
                    Err("refused".into())
                } else {
                    Ok(tries)
                }
            },
            |d| waited.push(d),
        );
        assert_eq!(r, Ok(3));
        assert_eq!(waited, WINDOWS_RETRY_DELAYS[..2]);
        // Windows, never working: every wait, then the last error.
        let mut tries = 0;
        let mut waited = Vec::new();
        let r: Result<(), String> = build_retrying(
            Platform::Windows,
            || {
                tries += 1;
                Err(format!("no {tries}"))
            },
            |d| waited.push(d),
        );
        assert_eq!(r, Err(format!("no {}", WINDOWS_RETRY_DELAYS.len() + 1)));
        assert_eq!(waited, WINDOWS_RETRY_DELAYS);
        let total: Duration = WINDOWS_RETRY_DELAYS.iter().sum();
        assert!(total >= Duration::from_secs(5) && total <= Duration::from_secs(10));
        // A first try that works waits for nothing.
        let mut waited = 0;
        assert_eq!(
            build_retrying(Platform::Windows, || Ok::<_, String>(1), |_| waited += 1),
            Ok(1)
        );
        assert_eq!(waited, 0);
    }

    #[test]
    fn the_sinkhole_counts_each_connection_and_closes_it() {
        let sinkhole = Sinkhole::bind_preferring(None).unwrap();
        assert_ne!(sinkhole.port(), 0);
        assert_eq!(sinkhole.attempts(), 0);
        for _ in 0..2 {
            let mut conn = TcpStream::connect((Ipv4Addr::LOCALHOST, sinkhole.port())).unwrap();
            conn.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut buf = [0u8; 16];
            // Closed without a byte written: end of stream, or a reset.
            assert!(matches!(conn.read(&mut buf), Ok(0) | Err(_)));
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while sinkhole.attempts() < 2 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(sinkhole.attempts(), 2);
    }

    #[test]
    fn the_top_frame_script_carries_pm_origin_and_the_dev_flag() {
        let linux = top_frame_script(Platform::Linux, false);
        assert!(linux.starts_with("(function (PM_TOP_FRAME) {\n"));
        assert!(linux.ends_with(r#"})({"origins":["tauri://localhost"],"dev":false});"#));
        assert!(linux.contains(TOP_FRAME_JS));
        let windows = top_frame_script(Platform::Windows, true);
        assert!(windows.ends_with(r#"})({"origins":["http://tauri.localhost"],"dev":true});"#));
    }

    #[test]
    fn the_top_frame_script_acts_only_in_the_top_frame_and_on_an_opaque_or_foreign_origin() {
        for needle in [
            "self === top",
            r#"String(self.origin) === "null""#,
            "PM_TOP_FRAME.origins.indexOf(pmOrigin) === -1",
            "window.stop()",
        ] {
            assert!(TOP_FRAME_JS.contains(needle), "{needle}");
        }
        assert_balanced(TOP_FRAME_JS);
    }

    #[test]
    fn the_webrtc_script_keys_off_an_opaque_origin_or_a_mod_url_and_leaves_media_alone() {
        for needle in [
            r#"String(self.origin) === "null""#,
            r#"location.protocol === "pmmod:""#,
            r#"location.hostname === "pmmod.localhost""#,
            "\"RTCPeerConnection\"",
            "\"webkitRTCPeerConnection\"",
            "\"RTCDataChannel\"",
            "\"RTCSessionDescription\"",
            "\"RTCIceCandidate\"",
            "configurable: false",
        ] {
            assert!(WEBRTC_OFF_SCRIPT.contains(needle), "{needle}");
        }
        let code = without_comment_lines(WEBRTC_OFF_SCRIPT);
        for absent in ["getUserMedia", "mediaDevices", "navigator"] {
            assert!(!code.contains(absent), "{absent}");
        }
        assert_balanced(WEBRTC_OFF_SCRIPT);
    }

    /// The scripts' comments are whole lines, and they name what the code leaves alone.
    fn without_comment_lines(js: &str) -> String {
        js.lines()
            .filter(|l| !l.trim_start().starts_with("//") && !l.trim_start().starts_with("/*"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// A cheap well-formedness check on the brackets outside strings and comments. The frontend test
    /// `src/lib/sealedEngineScripts.test.ts` parses and runs both scripts for real.
    fn assert_balanced(js: &str) {
        let mut stack = Vec::new();
        let mut chars = js.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '/' if chars.peek() == Some(&'/') => {
                    for c in chars.by_ref() {
                        if c == '\n' {
                            break;
                        }
                    }
                }
                '/' if chars.peek() == Some(&'*') => {
                    chars.next();
                    let mut prev = ' ';
                    for c in chars.by_ref() {
                        if prev == '*' && c == '/' {
                            break;
                        }
                        prev = c;
                    }
                }
                '"' => {
                    for c in chars.by_ref() {
                        if c == '"' {
                            break;
                        }
                    }
                }
                '(' | '[' | '{' => stack.push(c),
                ')' | ']' | '}' => {
                    let open = stack.pop();
                    let want = match c {
                        ')' => '(',
                        ']' => '[',
                        _ => '{',
                    };
                    assert_eq!(open, Some(want), "unbalanced at {c}");
                }
                _ => {}
            }
        }
        assert!(stack.is_empty(), "unclosed {stack:?}");
    }
}
