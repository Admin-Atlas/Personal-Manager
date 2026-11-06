// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Mods (spec `docs/mods-spec-2026-11-04.md`). So far: the sealed engine every PM window is built
//! with, and the `pmmod` scheme that serves a verified mod into a sealed frame (§1.2, §1.4).
//!
//! - [`engine`] holds the measured mitigations and the pure decisions, and [`headers`] the scheme's
//!   response headers; both are free of PM's crate so the sealed-frame regression app can include
//!   them.
//! - [`ids`]: the mod id grammar and the trust tiers. [`package`]: the `.pmmod` verifier.
//! - [`runtime`]: the installed-mod registry, the one-time entry URLs and the frame sessions, and
//!   the serve decision. This file's three commands are its IPC surface.
//! - [`store`]: the closed value sets of the v60 mod tables (`db/migrations.rs`).
//! - [`ledger`]: install, update, repair and uninstall, and the writer's startup load and sweep
//!   (§1.3); [`policy`] its pure decisions, [`disk`] the only code that writes under
//!   `data_dir/mods` (§1.3, §6).
//!
//! This file wires them into PM:
//! - [`EngineState`], managed from the first line of `setup()`: the sinkhole, the one set of WebView2
//!   args, and each switch's outcome, which say whether the engine is sealed.
//! - [`build_window`], the one way a PM window is built. It adds the navigation guard, the download
//!   refusal and both init scripts, passes the shared browser args, and applies the engine switches.
//! - [`build_main_window`]: the main window, from its `tauri.conf.json` entry (`"create": false`, so
//!   Tauri leaves it to `setup()`), which keeps every field it had when Tauri built it.
//!
//! The main window is the only one that will ever hold a mod frame, so only its switches count
//! towards the seal. The briefing panel (tray.rs) and the pop-out Calendar (popout.rs) host no mods
//! and get everything all the same, as defence in depth. On Windows they have to: WebView2 refuses a
//! second webview in the same user-data folder with different browser args, and every PM window
//! shares one (Tauri gives each the app's local data folder).

pub mod disk;
pub mod engine;
pub mod headers;
pub mod ids;
pub mod ledger;
pub mod package;
pub mod policy;
pub mod runtime;
pub mod store;

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard};

use tauri::http::{Request, StatusCode};
use tauri::webview::PageLoadEvent;
use tauri::{
    AppHandle, Manager, State, UriSchemeContext, UriSchemeResponder, Url, Webview, WebviewWindow,
    WebviewWindowBuilder, Wry,
};

use crate::error::{Error, Result};
use engine::{Platform, Sinkhole, SwitchOutcome};
use ids::ModId;
use runtime::{FrameOpened, ModRuntime};

/// The main window's label. `capabilities/default.json`, the tray and every `emit_to("main", …)`
/// name it too.
pub const MAIN_LABEL: &str = "main";

/// The custom scheme every mod is served from (`Platform::mod_origin`).
pub const SCHEME: &str = "pmmod";

/// The engine's seal, for the life of the process.
pub struct EngineState {
    is_dev: bool,
    /// `None` when the platform needs none (macOS), or binding loopback failed (recorded as a
    /// switch that didn't take).
    sinkhole: Option<Sinkhole>,
    /// Computed once, so every window gets the same.
    browser_args: String,
    outcomes: Mutex<Vec<SwitchOutcome>>,
    /// Whether the final verdict has been handed out (by [`EngineState::record`]), so it is logged
    /// once.
    verdict_told: AtomicBool,
}

/// The file in the app's local data folder that remembers the sinkhole's port between runs
/// (Windows only): WebView2's browser args carry it, and keeping it from one run to the next keeps
/// a relaunch's args equal to those of the WebView2 the last run may still have running.
const SINKHOLE_PORT_FILE: &str = "webview-proxy-port";

impl EngineState {
    /// [`EngineState::start`] with what this app and machine say: the remembered port's file, and on
    /// Windows whether something outside PM overrides WebView2's browser args.
    pub fn start_for(app: &AppHandle) -> Self {
        let on_windows = Platform::current() == Platform::Windows;
        let port_file = if on_windows {
            app.path()
                .app_local_data_dir()
                .ok()
                .map(|dir| dir.join(SINKHOLE_PORT_FILE))
        } else {
            None
        };
        let overridden = if on_windows {
            engine::webview2_args_override(
                std::env::var_os(engine::WEBVIEW2_ARGS_ENV).as_deref(),
                webview2_args_policy(&app.config().identifier).as_deref(),
            )
        } else {
            None
        };
        Self::start(tauri::is_dev(), port_file.as_deref(), overridden)
    }

    /// Bind the sinkhole (where the platform uses one), on the port `port_file` remembers when it
    /// is free, and remember the one it got. Called before any window is built: the browser args
    /// carry its port. `args_overridden` says why WebView2 may not use PM's args at all; the proxy
    /// switch then counts as not taken.
    pub fn start(is_dev: bool, port_file: Option<&Path>, args_overridden: Option<String>) -> Self {
        let platform = Platform::current();
        let mut outcomes = Vec::new();
        if let Some(why) = args_overridden {
            let failed = SwitchOutcome::failed(engine::WINDOWS_PROXY_ARGS, why);
            report_failure(&failed);
            outcomes.push(failed);
        }
        let remembered = port_file
            .and_then(|path| std::fs::read_to_string(path).ok())
            .and_then(|text| engine::parse_remembered_port(&text));
        let sinkhole = if platform.uses_sinkhole() {
            match Sinkhole::bind_preferring(remembered) {
                Ok(sinkhole) => {
                    outcomes.push(SwitchOutcome::took(engine::SINKHOLE));
                    if let Some(path) = port_file.filter(|_| remembered != Some(sinkhole.port())) {
                        remember_port(path, sinkhole.port());
                    }
                    Some(sinkhole)
                }
                Err(e) => {
                    let failed = SwitchOutcome::failed(
                        engine::SINKHOLE,
                        format!("couldn't listen on loopback: {e}"),
                    );
                    report_failure(&failed);
                    outcomes.push(failed);
                    None
                }
            }
        } else {
            None
        };
        let browser_args =
            engine::windows_browser_args(sinkhole.as_ref().map(Sinkhole::port), is_dev);
        Self {
            is_dev,
            sinkhole,
            browser_args,
            outcomes: Mutex::new(outcomes),
            verdict_told: AtomicBool::new(false),
        }
    }

    /// Whether every switch the engine depends on took. Until the main window has been built and
    /// sealed, it hasn't: unsealed is the starting state.
    #[allow(dead_code)] // read by the grant check (spec §1.4 item 3, a later slice); the tests read it today
    pub fn engine_sealed(&self) -> bool {
        self.verdict().sealed
    }

    /// Why the engine isn't sealed; empty when it is.
    #[allow(dead_code)] // read by the Developer tab (Mods PR-1, part 6); the tests read it today
    pub fn unsealed_reasons(&self) -> Vec<String> {
        self.verdict().reasons
    }

    /// Connections that reached the sinkhole since launch.
    #[allow(dead_code)] // read by the Developer tab (Mods PR-1, part 6); the tests read it today
    pub fn sinkhole_attempts(&self) -> u64 {
        self.sinkhole.as_ref().map_or(0, Sinkhole::attempts)
    }

    fn sinkhole_port(&self) -> Option<u16> {
        self.sinkhole.as_ref().map(Sinkhole::port)
    }

    fn verdict(&self) -> engine::Verdict {
        engine::verdict(Platform::current().expected_switches(), &self.outcomes())
    }

    /// Add `new` to the outcomes. The first time every expected switch has reported, the verdict is
    /// final, and it is handed back (once) to be logged: on macOS the content rule list reports from
    /// WebKit's completion handler after `setup()` has returned, so a verdict read at the end of the
    /// build would always say the list was never applied. A failure arriving later is still logged
    /// on its own, by `report_failure`.
    fn record(&self, new: Vec<SwitchOutcome>) -> Option<engine::Verdict> {
        for o in new.iter().filter(|o| !o.took) {
            report_failure(o);
        }
        let mut outcomes = self.outcomes();
        outcomes.extend(new);
        let expected = Platform::current().expected_switches();
        if !engine::unreported(expected, &outcomes).is_empty()
            || self.verdict_told.swap(true, Ordering::SeqCst)
        {
            return None;
        }
        Some(engine::verdict(expected, &outcomes))
    }

    /// A panic while holding this lock can only have come from `Vec::extend`; the list is still
    /// whole, so a poisoned lock is read through.
    fn outcomes(&self) -> MutexGuard<'_, Vec<SwitchOutcome>> {
        self.outcomes.lock().unwrap_or_else(|e| e.into_inner())
    }
}

fn report_failure(o: &SwitchOutcome) {
    eprintln!(
        "mods: the engine switch {} didn't take ({}), so the engine is unsealed",
        o.name, o.detail
    );
}

/// Best effort: a port that isn't remembered only means a fresh one next run.
fn remember_port(path: &Path, port: u16) {
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(path, port.to_string()));
    if let Err(e) = written {
        eprintln!("mods: couldn't remember the sinkhole's port: {e}");
    }
}

/// Where a WebView2 policy value that sets PM's browser args was found, if one exists: the
/// `AdditionalBrowserArguments` key under HKLM or HKCU, with a value named for PM's executable, its
/// identifier, or `*`. Any type and any content counts (fail closed).
#[cfg(windows)]
fn webview2_args_policy(identifier: &str) -> Option<String> {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{
        RegGetValueW, HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_ANY,
    };

    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.file_name().and_then(|n| n.to_str()).map(str::to_owned));
    let key = wide(engine::WEBVIEW2_ARGS_POLICY_KEY);
    let hives: [(HKEY, &str); 2] = [(HKEY_LOCAL_MACHINE, "HKLM"), (HKEY_CURRENT_USER, "HKCU")];
    for name in engine::webview2_policy_value_names(exe.as_deref(), identifier) {
        let value = wide(&name);
        for (hive, hive_name) in hives {
            // SAFETY: both strings are NUL-terminated UTF-16 that outlive the call, and no data
            // buffer is passed, so nothing is written: the call only says whether the value exists.
            let status = unsafe {
                RegGetValueW(
                    hive,
                    PCWSTR(key.as_ptr()),
                    PCWSTR(value.as_ptr()),
                    RRF_RT_ANY,
                    None,
                    None,
                    None,
                )
            };
            if status == ERROR_SUCCESS {
                return Some(format!(
                    "{hive_name}\\{}, value {name}",
                    engine::WEBVIEW2_ARGS_POLICY_KEY
                ));
            }
        }
    }
    None
}

/// No WebView2, no policy.
#[cfg(not(windows))]
fn webview2_args_policy(_identifier: &str) -> Option<String> {
    None
}

/// What a window may hold. Only the main window ever holds a mod frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    Main,
    NoMods,
}

/// Build the main window from its `tauri.conf.json` entry, sealed. Called once, at the top of
/// `setup()`, which is where Tauri itself built it before (`tauri::app::setup` builds every
/// `create: true` window just before calling the setup hook). `build_window` applies its switches
/// there, on the main thread `setup()` runs on. On Linux and Windows they are all in place when it
/// returns. On macOS only the content rule list's compile has started: WebKit compiles and adds it
/// later, and the engine stays unsealed until it reports.
///
/// On Windows a failed build is tried again for a few seconds (`engine::build_retrying`): when PM
/// relaunches, the last run's WebView2 may still be running with other browser args, and WebView2
/// refuses the window until it exits. Without the retry, `setup()` would fail and PM would quit.
/// Each try builds the window hidden (`engine::hidden_until_built`), so a refused try never puts a
/// blank window on screen. The window is shown once it is built and its switches are applied or, on
/// macOS, started: there it is on screen before the rule list is in place. PM's own page makes no
/// web loads, and the grant check (spec §1.4 item 3, a later slice) is to read
/// `EngineState::engine_sealed`, which says unsealed until the list reports.
///
/// The verdict is logged by `settle` once every switch has reported, not here: macOS's rule list
/// reports after `setup()` returns.
pub fn build_main_window(app: &AppHandle) -> Result<WebviewWindow> {
    let config = app
        .config()
        .app
        .windows
        .iter()
        .find(|w| w.label == MAIN_LABEL)
        .ok_or_else(|| Error::Other("tauri.conf.json has no main window".into()))?;
    let (hidden, show) = engine::hidden_until_built(config);
    let window = engine::build_retrying(
        Platform::current(),
        || {
            let builder = WebviewWindowBuilder::from_config(app, &hidden)
                .map_err(|e| Error::Other(e.to_string()))?;
            build_window(builder, app, Role::Main)
        },
        std::thread::sleep,
    )?;
    if show {
        window.show().map_err(|e| Error::Other(e.to_string()))?;
    }
    Ok(window)
}

/// Build a PM window, sealed. Every PM window goes through here (INVARIANTS.md I-26): the guard, the
/// download refusal, both init scripts, the shared browser args and the engine switches.
pub fn build_window(
    builder: WebviewWindowBuilder<'_, Wry, AppHandle>,
    app: &AppHandle,
    role: Role,
) -> Result<WebviewWindow> {
    let state = app
        .try_state::<EngineState>()
        .ok_or_else(|| Error::Other("PM's webview engine isn't set up yet".into()))?;
    let builder = if role == Role::Main {
        let reload_app = app.clone();
        // A new page in main (a reload, the frozen-mod watchdog's or a dev one) unmounts every
        // ModFrame without its cleanup running, so nothing would ever close their sessions: end
        // them all here. Only the top document's loads arrive (WebKitGTK's `load-changed`,
        // WKWebView's committed navigations, WebView2's `ContentLoading`), never a frame's, and
        // each arrives before the new page's own scripts run.
        builder.on_page_load(move |_, payload| {
            if payload.event() == PageLoadEvent::Started {
                if let Some(runtime) = reload_app.try_state::<ModRuntime>() {
                    runtime.close_all();
                }
            }
        })
    } else {
        builder
    };
    let guard_app = app.clone();
    // The wiring is engine.rs's, so the sealed-frame regression app builds its window the same way.
    let window = engine::seal_builder(builder, &state.browser_args, state.is_dev, move |url| {
        role == Role::Main && is_pending_entry(&guard_app, url)
    })
    .build()
    .map_err(|e| Error::Other(e.to_string()))?;
    seal_webview(app, &window, &state, role);
    Ok(window)
}

/// Whether `url` is a one-time mod entry URL that a mod frame is about to load (spec §1.4, defence
/// 1): the only `pmmod` URL the guard lets a frame navigate to. The runtime's table of minted entry
/// URLs answers it without consuming anything; the scheme handler consumes the token when the
/// frame's request arrives. Before the runtime is managed, nothing is pending.
fn is_pending_entry(app: &AppHandle, url: &Url) -> bool {
    app.try_state::<ModRuntime>()
        .is_some_and(|runtime| runtime.is_pending_entry(url))
}

/// The scheme's runtime, managed by `setup()` before the main window is built.
pub fn start_runtime(app: &AppHandle) -> ModRuntime {
    let dev_url = if tauri::is_dev() {
        app.config().build.dev_url.clone()
    } else {
        None
    };
    let platform = Platform::current();
    ModRuntime::new(
        platform,
        runtime::frame_ancestors(platform, dev_url.as_ref()),
    )
}

/// The `pmmod` scheme handler, registered on the Builder. The file is read off the thread the
/// engine called from (WebKitGTK calls on the main thread), and only a few at a time
/// (`runtime::ServeGate`): every request reads and hashes its whole file, and a mod can ask for
/// hundreds at once. The rest wait their turn as small futures, holding no thread and no buffer.
pub fn handle_scheme_request(
    ctx: UriSchemeContext<'_, Wry>,
    req: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let app = ctx.app_handle().clone();
    let label = ctx.webview_label().to_owned();
    tauri::async_runtime::spawn(async move {
        let Some(runtime) = app.try_state::<ModRuntime>() else {
            responder.respond(runtime::refusal(StatusCode::NOT_FOUND));
            return;
        };
        let Some(turn) = runtime.gate().enter().await else {
            responder.respond(runtime::refusal(StatusCode::SERVICE_UNAVAILABLE));
            return;
        };
        let reader = app.clone();
        let served = tauri::async_runtime::spawn_blocking(move || {
            reader
                .try_state::<ModRuntime>()
                .map(|runtime| runtime.serve(&label, &req))
        })
        .await;
        drop(turn);
        responder.respond(match served {
            Ok(Some(response)) => response,
            Ok(None) => runtime::refusal(StatusCode::NOT_FOUND),
            Err(_) => runtime::refusal(StatusCode::INTERNAL_SERVER_ERROR),
        });
    });
}

// ---- Commands ----------------------------------------------------------------------------------

/// Mods live in the main window only (spec §1.5's first check).
fn main_only(webview: &Webview) -> Result<()> {
    main_only_label(webview.label())
}

/// [`main_only`]'s decision: exactly the main window's label, nothing like it.
fn main_only_label(label: &str) -> Result<()> {
    if label == MAIN_LABEL {
        Ok(())
    } else {
        Err(Error::Other("Mods run only in PM's main window.".into()))
    }
}

/// Mint a one-time entry URL for a mod's frame and the session its bridge calls under.
#[tauri::command]
pub fn mod_frame_open(
    webview: Webview,
    runtime: State<'_, ModRuntime>,
    mod_id: String,
) -> Result<FrameOpened> {
    main_only(&webview)?;
    let id = ModId::parse(&mod_id).map_err(|e| Error::Other(e.to_string()))?;
    runtime
        .open_frame(&id)
        .map_err(|e| Error::Other(e.to_string()))
}

/// End a frame's session (its frame unmounted, or navigated). Ending an ended one is fine.
#[tauri::command]
pub fn mod_frame_close(
    webview: Webview,
    runtime: State<'_, ModRuntime>,
    session: String,
) -> Result<()> {
    main_only(&webview)?;
    runtime.close(&session);
    Ok(())
}

/// One call from a mod's frame, relayed by PM's page. The mod is the session's, never anything the
/// frame said.
#[tauri::command]
pub fn mod_call(
    webview: Webview,
    runtime: State<'_, ModRuntime>,
    session: String,
    method: String,
    args: serde_json::Value,
) -> Result<serde_json::Value> {
    main_only(&webview)?;
    runtime
        .call(&session, &method, &args)
        .map_err(|e| Error::Other(e.to_string()))
}

/// Apply the engine switches to a freshly built window and, for the main window, record whether
/// each took. A failure to reach the webview is logged, never fatal: PM still works, and the main
/// window's switches then never report, so the engine stays unsealed.
fn seal_webview(app: &AppHandle, window: &WebviewWindow, state: &EngineState, role: Role) {
    let record = role == Role::Main;
    let settle_app = app.clone();
    if let Err(e) = engine::apply_switches(
        window,
        state.sinkhole_port(),
        state.is_dev,
        move |outcomes| settle(&settle_app, outcomes, record),
    ) {
        let consequence = if record {
            ", so the engine stays unsealed"
        } else {
            ""
        };
        eprintln!("mods: couldn't reach the webview to seal it ({e}){consequence}");
    }
}

/// Where a window's outcomes go: into the seal for the main window, the log otherwise. The main
/// window's verdict is logged when it is final, if it is unsealed.
fn settle(app: &AppHandle, outcomes: Vec<SwitchOutcome>, record: bool) {
    if record {
        let state = app.try_state::<EngineState>();
        if let Some(verdict) = state.and_then(|state| state.record(outcomes)) {
            if !verdict.sealed {
                eprintln!(
                    "mods: the webview engine isn't sealed: {}",
                    verdict.reasons.join("; ")
                );
            }
        }
        return;
    }
    for o in outcomes.iter().filter(|o| !o.took) {
        eprintln!(
            "webview: the engine switch {} didn't take in a window without mods ({})",
            o.name, o.detail
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_main_windows_label_may_use_the_mod_commands() {
        assert!(main_only_label(MAIN_LABEL).is_ok());
        for label in [
            "calendar", "briefing", "", "Main", "main ", " main", "main\0", "mainx",
        ] {
            let refused = main_only_label(label).unwrap_err().to_string();
            assert!(
                refused.contains("only in PM's main window"),
                "{label:?}: {refused}"
            );
        }
    }

    #[test]
    fn overridden_webview2_args_keep_the_engine_unsealed_whatever_else_takes() {
        let state = EngineState::start(false, None, Some("a policy sets them".into()));
        let all: Vec<_> = Platform::current()
            .expected_switches()
            .iter()
            .copied()
            .chain([engine::WINDOWS_PROXY_ARGS])
            .map(SwitchOutcome::took)
            .collect();
        state.record(all);
        assert!(!state.engine_sealed());
        assert!(state
            .unsealed_reasons()
            .iter()
            .any(|r| r.contains("webview2.proxy-args didn't take: a policy sets them")));
    }

    #[test]
    fn the_sinkhole_port_is_remembered_and_reused_where_one_is_kept() {
        if !Platform::current().uses_sinkhole() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("nested").join(SINKHOLE_PORT_FILE);
        let first = EngineState::start(false, Some(&file), None);
        let port = first.sinkhole_port().unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), port.to_string());
        // Still held by `first`: the next run gets a fresh port and remembers that one.
        let second = EngineState::start(false, Some(&file), None);
        let second_port = second.sinkhole_port().unwrap();
        assert_ne!(second_port, port);
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            second_port.to_string()
        );
        // A file that names no usable port is replaced, never trusted.
        std::fs::write(&file, "80").unwrap();
        let third = EngineState::start(false, Some(&file), None);
        assert_ne!(third.sinkhole_port(), Some(80));
        assert_eq!(
            std::fs::read_to_string(&file).unwrap(),
            third.sinkhole_port().unwrap().to_string()
        );
    }

    #[test]
    fn a_fresh_engine_is_unsealed_until_every_switch_reports() {
        let state = EngineState::start(false, None, None);
        assert!(!state.engine_sealed());
        assert!(!state.unsealed_reasons().is_empty());
        // Where this platform has a sinkhole, `start` has reported it already.
        let expected = Platform::current().expected_switches();
        let mut rest: Vec<_> = expected
            .iter()
            .copied()
            .filter(|n| *n != engine::SINKHOLE)
            .map(SwitchOutcome::took)
            .collect();
        let last = rest.pop().unwrap();
        // Not final while one is still on its way (macOS's rule list, after setup() returns).
        assert_eq!(state.record(rest), None);
        assert!(!state.engine_sealed());
        // The last report makes it final, and it is handed out once.
        let told = state.record(vec![last]).expect("the verdict is final");
        assert!(told.sealed, "{:?}", told.reasons);
        assert!(state.engine_sealed(), "{:?}", state.unsealed_reasons());
        assert_eq!(state.sinkhole_attempts(), 0);
        // One late failure unseals it again; that failure is logged on its own, not as a verdict.
        assert_eq!(
            state.record(vec![SwitchOutcome::failed(expected[0], "test")]),
            None
        );
        assert!(!state.engine_sealed());
    }

    #[test]
    fn a_final_verdict_that_is_unsealed_says_why() {
        let state = EngineState::start(false, None, None);
        let expected = Platform::current().expected_switches();
        let last = *expected.last().unwrap();
        let mut outcomes: Vec<_> = expected
            .iter()
            .copied()
            .filter(|n| *n != engine::SINKHOLE && *n != last)
            .map(SwitchOutcome::took)
            .collect();
        outcomes.push(SwitchOutcome::failed(last, "WebKit didn't compile it"));
        let told = state.record(outcomes).expect("every switch reported");
        assert!(!told.sealed);
        assert_eq!(
            told.reasons,
            [format!("{last} didn't take: WebKit didn't compile it")]
        );
        assert_eq!(state.record(vec![SwitchOutcome::took(last)]), None);
    }

    /// The dev build's exemptions in macOS's rule list, run against URLs the way WebKit runs them:
    /// each filter searched in the whole URL, case-insensitively, in order, an
    /// `ignore-previous-rules` match cancelling the blocks before it. WebKit's rule regexes are a
    /// subset of the `regex` crate's syntax, with the same meaning. (Here rather than in engine.rs,
    /// whose tests also build in the regression app, which has no regex crate.)
    #[test]
    fn the_macos_rule_list_exempts_vite_by_its_host_never_by_a_prefix() {
        let blocked_by = |is_dev: bool, url: &str| {
            let rules: serde_json::Value =
                serde_json::from_str(&engine::content_rules_json(is_dev)).unwrap();
            let mut blocked = false;
            for rule in rules.as_array().unwrap() {
                let filter = rule["trigger"]["url-filter"].as_str().unwrap();
                let re = regex::RegexBuilder::new(filter)
                    .case_insensitive(true)
                    .build()
                    .unwrap();
                if re.is_match(url) {
                    blocked = rule["action"]["type"] == "block";
                }
            }
            blocked
        };
        for url in [
            "http://localhost:1420/",
            "http://localhost/",
            "http://127.0.0.1:1420/src/main.tsx",
            "ws://localhost:1420/?token=x",
            "wss://127.0.0.1:1420/",
            "http://localhost:1420/@evil.example/",
        ] {
            assert!(!blocked_by(true, url), "dev, Vite: {url}");
            assert!(blocked_by(false, url), "release: {url}");
        }
        for url in [
            // Userinfo naming Vite's host: the host is evil.example.
            "http://localhost:1@evil.example/",
            "http://localhost@evil.example/",
            "http://localhost:1420@evil.example/",
            "ws://localhost:1@evil.example/",
            "http://127.0.0.1:1@evil.example/",
            "wss://127.0.0.1@evil.example/",
            // Hosts that start like Vite's.
            "http://localhost.evil.example/",
            "http://localhostevil.example/",
            "http://127.0.0.1.evil.example/",
            "http://127.0.0.10/",
            "https://example.com/",
            "http://xn--1ug.pm-probe.test:8080/x",
        ] {
            assert!(blocked_by(true, url), "dev: {url}");
            assert!(blocked_by(false, url), "release: {url}");
        }
        for own in [
            "tauri://localhost/",
            "pmmod://localhost/a.b/app/index.html",
            "ipc://localhost/x",
        ] {
            assert!(!blocked_by(false, own), "{own}");
        }
    }

    #[test]
    fn the_browser_args_carry_this_engines_sinkhole() {
        let state = EngineState::start(true, None, None);
        let port = state.sinkhole_port();
        assert_eq!(Platform::current().uses_sinkhole(), port.is_some());
        assert_eq!(state.browser_args, engine::windows_browser_args(port, true));
    }

    fn conf() -> serde_json::Value {
        serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap()
    }

    /// `setup()` builds the main window (sealed), so Tauri mustn't: a window Tauri built would have
    /// no guard, no init scripts, and on Windows browser args the pop-out's could never match.
    #[test]
    fn tauri_conf_leaves_the_main_window_to_setup() {
        let conf = conf();
        let windows = conf["app"]["windows"].as_array().unwrap();
        assert_eq!(windows.len(), 1);
        let main = &windows[0];
        assert_eq!(main["label"], MAIN_LABEL);
        assert_eq!(main["create"], false);
        // `Platform::app_origin` is Tauri's origin with `useHttpsScheme` off.
        assert!(main.get("useHttpsScheme").is_none_or(|v| v == false));
    }

    fn directive<'a>(csp: &'a str, name: &str) -> &'a str {
        csp.split(';')
            .map(str::trim)
            .find(|d| d.split_whitespace().next() == Some(name))
            .unwrap_or_else(|| panic!("no {name} in {csp}"))
    }

    /// Main may frame mods and nothing else (spec §1.4, "The main CSP change"), in both the release
    /// and the dev CSP; `asset:` is gone (the `protocol-asset` feature is off and nothing calls
    /// `convertFileSrc`), and the page still runs no inline script.
    #[test]
    fn the_csp_frames_only_mods() {
        let conf = conf();
        for key in ["csp", "devCsp"] {
            let csp = conf["app"]["security"][key].as_str().unwrap();
            assert_eq!(
                directive(csp, "frame-src"),
                format!(
                    "frame-src {} {}",
                    Platform::Linux.mod_origin(),
                    Platform::Windows.mod_origin()
                ),
                "{key}"
            );
            assert_eq!(Platform::MacOs.mod_origin(), Platform::Linux.mod_origin());
            assert!(!csp.contains("asset:"), "{key}");
            assert_eq!(
                directive(csp, "img-src"),
                "img-src 'self' data: blob:",
                "{key}"
            );
            assert!(
                directive(csp, "script-src").starts_with("script-src 'self'"),
                "{key}"
            );
            assert!(
                !directive(csp, "script-src").contains("'unsafe-inline'"),
                "{key}"
            );
            assert_eq!(directive(csp, "object-src"), "object-src 'none'", "{key}");
        }
    }
}
