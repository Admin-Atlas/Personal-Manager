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
//! - [`EngineState`], managed from the first line of `setup()`: the sinkhole, the proxy port every
//!   window is pointed at (the dead one when the sinkhole couldn't be bound), the one set of WebView2
//!   args, and each switch's outcome, which say whether the engine is sealed.
//! - [`build_window`], the one way a PM window is built. It adds the navigation guard, the download
//!   refusal and both init scripts, passes the shared browser args, and applies the engine switches.
//! - [`build_main_window`]: the main window, from its `tauri.conf.json` entry (`"create": false`, so
//!   Tauri leaves it to `setup()`), which keeps every field it had when Tauri built it.
//!
//! The main window is the only one that will ever hold a mod frame, so only its switches count
//! towards the seal, but for one kind: a switch whose setting the engine shares between every
//! webview in the user-data folder (WebView2's SmartScreen, `engine::WINDOWS_REPUTATION_OFF`)
//! fails the seal from any window, since one window left at the default turns it back on in main
//! too ([`for_the_seal`]). The briefing panel (tray.rs) and the pop-out Calendar (popout.rs) host
//! no mods and get everything all the same, as defence in depth. On Windows they have to: WebView2
//! refuses a second webview in the same user-data folder with different browser args, and every PM
//! window shares one (Tauri gives each the app's local data folder).

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
    /// The engine whose switches are expected: [`Platform::current`], but for a test's.
    platform: Platform,
    is_dev: bool,
    /// `None` when the platform needs none (macOS), or binding loopback failed (recorded as a
    /// switch that didn't take).
    sinkhole: Option<Sinkhole>,
    /// Where every window's proxy points ([`engine::proxy_port`]): the sinkhole's port, or the dead
    /// port when it couldn't be bound, so a failed bind never leaves a window a direct route out.
    /// `None` only on macOS, which has no proxy.
    proxy_port: Option<u16>,
    /// Computed once, from `proxy_port`, so every window gets the same.
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
    ///
    /// A bind that still fails after [`engine::bind_retrying`]'s tries fails closed: the sinkhole
    /// switch reports failed, so the engine is unsealed, and every window's proxy is the dead port
    /// instead, which is never remembered (the next run tries the remembered port again).
    pub fn start(is_dev: bool, port_file: Option<&Path>, args_overridden: Option<String>) -> Self {
        Self::start_with(
            Platform::current(),
            is_dev,
            port_file,
            args_overridden,
            |preferred| {
                engine::bind_retrying(|| Sinkhole::bind_preferring(preferred), std::thread::sleep)
            },
        )
    }

    /// [`EngineState::start`] for `platform`'s engine, binding the sinkhole with `bind`, which is
    /// handed the remembered port, so a test can make the bind fail or judge another engine's
    /// switches.
    fn start_with(
        platform: Platform,
        is_dev: bool,
        port_file: Option<&Path>,
        args_overridden: Option<String>,
        bind: impl FnOnce(Option<u16>) -> std::io::Result<Sinkhole>,
    ) -> Self {
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
            match bind(remembered) {
                Ok(sinkhole) => {
                    outcomes.push(SwitchOutcome::took(engine::SINKHOLE));
                    if let Some(path) = port_file.filter(|_| remembered != Some(sinkhole.port())) {
                        remember_port(path, sinkhole.port());
                    }
                    Some(sinkhole)
                }
                Err(e) => {
                    let failed = engine::sinkhole_unbound(&e);
                    report_failure(&failed);
                    outcomes.push(failed);
                    None
                }
            }
        } else {
            None
        };
        let proxy_port = engine::proxy_port(platform, sinkhole.as_ref().map(Sinkhole::port));
        let browser_args = engine::windows_browser_args(proxy_port, is_dev);
        Self {
            platform,
            is_dev,
            sinkhole,
            proxy_port,
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

    /// What every window's switches are applied with: [`engine::proxy_port`]'s port, the dead one
    /// included.
    fn proxy_port(&self) -> Option<u16> {
        self.proxy_port
    }

    #[cfg(test)]
    fn sinkhole_port(&self) -> Option<u16> {
        self.sinkhole.as_ref().map(Sinkhole::port)
    }

    fn verdict(&self) -> engine::Verdict {
        engine::verdict(self.platform.expected_switches(), &self.outcomes())
    }

    /// Take a window's switch report (`label`'s, built as `role`): what [`for_the_seal`] counts goes
    /// into the seal, and any other failure is logged. The verdict is logged once it is final, if it
    /// is unsealed.
    fn settle(&self, role: Role, label: &str, outcomes: Vec<SwitchOutcome>) {
        let (counted, logged) = for_the_seal(self.platform, role, label, outcomes);
        for o in logged.iter().filter(|o| !o.took) {
            eprintln!(
                "webview: the engine switch {} didn't take in the {label} window, which holds no \
                 mods ({})",
                o.name, o.detail
            );
        }
        if counted.is_empty() {
            return;
        }
        if let Some(verdict) = self.record(counted) {
            if !verdict.sealed {
                eprintln!(
                    "mods: the webview engine isn't sealed: {}",
                    verdict.reasons.join("; ")
                );
            }
        }
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
        let expected = self.platform.expected_switches();
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

/// Apply the engine switches to a freshly built window and settle what took
/// ([`EngineState::settle`]). A failure to reach the webview is logged, never fatal: PM still works.
/// The main window's switches then never report, so the engine stays unsealed; another window's
/// folder-wide switches count as failed ([`unreached`]).
fn seal_webview(app: &AppHandle, window: &WebviewWindow, state: &EngineState, role: Role) {
    let label = window.label().to_owned();
    let settle_app = app.clone();
    let settle_label = label.clone();
    let applied =
        engine::apply_switches(window, state.proxy_port(), state.is_dev, move |outcomes| {
            if let Some(state) = settle_app.try_state::<EngineState>() {
                state.settle(role, &settle_label, outcomes);
            }
        });
    if let Err(e) = applied {
        let consequence = if role == Role::Main {
            ", so the engine stays unsealed"
        } else {
            ""
        };
        eprintln!(
            "mods: couldn't reach the {label} window's webview to seal it ({e}){consequence}"
        );
        state.settle(
            role,
            &label,
            unreached(state.platform, role, &e.to_string()),
        );
    }
}

/// Which of a window's switch outcomes go into the seal, and which are only logged: `(counted,
/// logged)`. `label` names the window, and `role` says whether it is the main one.
///
/// The main window's all count: it is the one window that holds mods. Another window's count only
/// where they are failures of a switch whose setting the engine shares between every webview in
/// the user-data folder ([`Platform::folder_wide_switches`]): WebView2 turns SmartScreen on for
/// every PM window if it is on in any of them, so a pop-out that couldn't switch it off has turned
/// it back on in the main window too, and the seal can't stand on the main window's report alone.
/// Such a failure says which window it was in.
///
/// Another window's successes never count, so a window without mods can unseal the engine but
/// never seal it: only the main window's own report can make a switch count as taken, and while it
/// hasn't reported, the switch is still never applied. A failure that comes after the verdict (a
/// pop-out opened later) unseals the engine for the rest of the run, even once that window has
/// closed: PM can't tell when WebView2 next works the setting out, and a WebView2 that refused it
/// in one window would likely refuse it in the next.
fn for_the_seal(
    platform: Platform,
    role: Role,
    label: &str,
    outcomes: Vec<SwitchOutcome>,
) -> (Vec<SwitchOutcome>, Vec<SwitchOutcome>) {
    if role == Role::Main {
        return (outcomes, Vec::new());
    }
    let folder_wide = platform.folder_wide_switches();
    let (mut counted, logged): (Vec<_>, Vec<_>) = outcomes
        .into_iter()
        .partition(|o| !o.took && folder_wide.contains(&o.name));
    for o in &mut counted {
        o.detail = format!("in the {label} window: {}", o.detail);
    }
    (counted, logged)
}

/// What a window whose webview couldn't be reached to apply its switches (`why`) reports. Nothing
/// for the main window: its switches then never report, which keeps the engine unsealed. For
/// another window, a failure of each folder-wide switch, since its webview is left at the engine's
/// default, which turns that setting back on for every window ([`for_the_seal`]).
fn unreached(platform: Platform, role: Role, why: &str) -> Vec<SwitchOutcome> {
    if role == Role::Main {
        return Vec::new();
    }
    platform
        .folder_wide_switches()
        .iter()
        .map(|&name| SwitchOutcome::failed(name, format!("couldn't reach the webview: {why}")))
        .collect()
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
        assert_eq!(state.proxy_port(), port);
        assert_eq!(state.browser_args, engine::windows_browser_args(port, true));
    }

    /// A sinkhole that can't be bound fails closed: every window's proxy is the dead port, in the
    /// browser args and in what the switches are applied with, never nothing (a direct route out);
    /// the engine is unsealed whatever else takes, and says where the traffic goes; and nothing is
    /// remembered for the next run.
    #[test]
    fn a_sinkhole_that_cant_bind_leaves_every_window_on_the_dead_proxy_and_the_engine_unsealed() {
        let dir = tempfile::tempdir().unwrap();
        let fresh = dir.path().join("nested").join(SINKHOLE_PORT_FILE);
        let kept = dir.path().join(SINKHOLE_PORT_FILE);
        std::fs::write(&kept, "40000").unwrap();
        let platform = Platform::current();
        for (file, remembered) in [(&fresh, None), (&kept, Some(40000))] {
            for is_dev in [false, true] {
                let mut asked = Vec::new();
                let state =
                    EngineState::start_with(platform, is_dev, Some(file), None, |preferred| {
                        asked.push(preferred);
                        Err(std::io::Error::other("refused by the test"))
                    });
                assert!(!state.engine_sealed());
                assert_eq!(state.sinkhole_attempts(), 0);
                if !platform.uses_sinkhole() {
                    // macOS: nothing to bind, and no proxy.
                    assert!(asked.is_empty());
                    assert_eq!(state.proxy_port(), None);
                    assert_eq!(state.browser_args, engine::WRY_DEFAULT_BROWSER_ARGS);
                    continue;
                }
                assert_eq!(asked, [remembered]);
                assert_eq!(state.sinkhole_port(), None);
                assert_eq!(state.proxy_port(), Some(engine::DEAD_PROXY_PORT));
                assert_eq!(
                    state.browser_args,
                    engine::windows_browser_args(Some(engine::DEAD_PROXY_PORT), is_dev)
                );
                assert!(state
                    .browser_args
                    .contains(" --proxy-server=http://127.0.0.1:1"));
                assert_eq!(state.browser_args.contains("<-loopback>"), !is_dev);
                // What the switches get: WebKitGTK's proxy points at the dead port, and WebView2's
                // args switch counts it as carried (the sinkhole's own switch is the one that fails).
                assert_eq!(
                    state
                        .proxy_port()
                        .map(engine::sinkhole_proxy_uri)
                        .as_deref(),
                    Some("http://127.0.0.1:1")
                );
                assert_eq!(
                    engine::proxy_args_outcome(state.proxy_port()),
                    SwitchOutcome::took(engine::WINDOWS_PROXY_ARGS)
                );
                let rest: Vec<_> = platform
                    .expected_switches()
                    .iter()
                    .copied()
                    .filter(|n| *n != engine::SINKHOLE)
                    .map(SwitchOutcome::took)
                    .collect();
                let told = state.record(rest).expect("every switch reported");
                assert!(!told.sealed);
                assert_eq!(
                    told.reasons,
                    [
                        "sinkhole didn't take: couldn't listen on loopback: refused by the test; \
                         the windows' traffic goes to 127.0.0.1:1 instead, a dead port, so it \
                         fails rather than going out directly"
                    ]
                );
            }
        }
        // Nothing remembered: no file (nor its folder) made, and the remembered port kept as it was.
        assert!(!fresh.parent().unwrap().exists());
        assert_eq!(std::fs::read_to_string(&kept).unwrap(), "40000");
    }

    /// An engine judged as `platform`'s, with a real sinkhole where it has one (so on Windows only
    /// the two switches the main window reports are still owed).
    fn engine_of(platform: Platform) -> EngineState {
        EngineState::start_with(platform, false, None, None, Sinkhole::bind_preferring)
    }

    #[test]
    fn a_window_without_mods_counts_only_where_it_failed_a_switch_webview2_shares() {
        use engine::{WINDOWS_PROXY_ARGS, WINDOWS_REPUTATION_OFF};
        let took = SwitchOutcome::took;
        let failed = SwitchOutcome::failed;
        let report = vec![
            took(WINDOWS_PROXY_ARGS),
            failed(WINDOWS_REPUTATION_OFF, "refused"),
        ];
        // The main window's report all counts, as it is, wherever it is.
        for platform in [Platform::Linux, Platform::MacOs, Platform::Windows] {
            assert_eq!(
                for_the_seal(platform, Role::Main, MAIN_LABEL, report.clone()),
                (report.clone(), vec![]),
                "{platform:?}"
            );
        }
        // Another window on Windows: only SmartScreen's failure counts, and says where it was.
        assert_eq!(
            for_the_seal(Platform::Windows, Role::NoMods, "calendar", report),
            (
                vec![failed(
                    WINDOWS_REPUTATION_OFF,
                    "in the calendar window: refused"
                )],
                vec![took(WINDOWS_PROXY_ARGS)]
            )
        );
        // Its successes never count, nor does a failure of a switch each window holds on its own.
        for logged in [
            vec![took(WINDOWS_PROXY_ARGS), took(WINDOWS_REPUTATION_OFF)],
            vec![
                failed(WINDOWS_PROXY_ARGS, "the args carry no proxy"),
                took(WINDOWS_REPUTATION_OFF),
            ],
        ] {
            assert_eq!(
                for_the_seal(Platform::Windows, Role::NoMods, "briefing", logged.clone()),
                (vec![], logged)
            );
        }
        // Linux and macOS share no such setting: another window's failures are only logged.
        for platform in [Platform::Linux, Platform::MacOs] {
            let all_failed: Vec<_> = [Platform::Linux, Platform::MacOs, Platform::Windows]
                .iter()
                .flat_map(|p| p.expected_switches().iter().copied())
                .map(|name| failed(name, "no"))
                .collect();
            assert_eq!(
                for_the_seal(platform, Role::NoMods, "calendar", all_failed.clone()),
                (vec![], all_failed),
                "{platform:?}"
            );
            assert!(unreached(platform, Role::NoMods, "gone").is_empty());
        }
        // A window that couldn't be reached is left at WebView2's default: SmartScreen on. The main
        // window reports nothing then, so its switches stay never applied.
        assert_eq!(
            unreached(Platform::Windows, Role::NoMods, "the event loop is gone"),
            [failed(
                WINDOWS_REPUTATION_OFF,
                "couldn't reach the webview: the event loop is gone"
            )]
        );
        assert!(unreached(Platform::Windows, Role::Main, "gone").is_empty());
    }

    /// Through the seal itself (`EngineState::settle`, `record`, the verdict), judged as Windows.
    #[test]
    fn on_windows_smartscreen_failing_in_any_window_unseals_the_engine() {
        use engine::{WINDOWS_PROXY_ARGS, WINDOWS_REPUTATION_OFF};
        let took = SwitchOutcome::took;
        let failed = SwitchOutcome::failed;
        let main_report = || vec![took(WINDOWS_PROXY_ARGS), took(WINDOWS_REPUTATION_OFF)];

        // Sealed by the main window's report, then unsealed by a pop-out's failure, which says where.
        let state = engine_of(Platform::Windows);
        state.settle(Role::Main, MAIN_LABEL, main_report());
        assert!(state.engine_sealed(), "{:?}", state.unsealed_reasons());
        state.settle(
            Role::NoMods,
            "calendar",
            vec![
                took(WINDOWS_PROXY_ARGS),
                failed(WINDOWS_REPUTATION_OFF, "refused"),
            ],
        );
        assert!(!state.engine_sealed());
        assert_eq!(
            state.unsealed_reasons(),
            ["webview2.reputation-checking-off didn't take: in the calendar window: refused"]
        );

        // The same before the main window has reported: the verdict, once final, is unsealed.
        let state = engine_of(Platform::Windows);
        state.settle(
            Role::NoMods,
            "briefing",
            unreached(Platform::Windows, Role::NoMods, "the event loop is gone"),
        );
        assert!(!state.engine_sealed());
        let told = state
            .record(main_report())
            .expect("every switch has reported");
        assert!(!told.sealed);
        assert_eq!(
            told.reasons,
            [
                "webview2.reputation-checking-off didn't take: in the briefing window: couldn't \
                 reach the webview: the event loop is gone"
            ]
        );

        // A pop-out's successes can't stand in for the main window's report.
        let state = engine_of(Platform::Windows);
        state.settle(Role::NoMods, "calendar", main_report());
        assert_eq!(
            state.unsealed_reasons(),
            [
                "webview2.proxy-args was never applied",
                "webview2.reputation-checking-off was never applied"
            ]
        );
        // Nor can a main window that couldn't be reached report anything.
        state.settle(
            Role::Main,
            MAIN_LABEL,
            unreached(Platform::Windows, Role::Main, "gone"),
        );
        assert_eq!(state.unsealed_reasons().len(), 2);

        // On Linux the pop-out's failures are its own: the engine stays sealed.
        let state = engine_of(Platform::Linux);
        let rest: Vec<_> = Platform::Linux
            .expected_switches()
            .iter()
            .copied()
            .filter(|n| *n != engine::SINKHOLE)
            .map(took)
            .collect();
        state.settle(Role::Main, MAIN_LABEL, rest.clone());
        assert!(state.engine_sealed(), "{:?}", state.unsealed_reasons());
        let all_failed = rest.iter().map(|o| failed(o.name, "no")).collect();
        state.settle(Role::NoMods, "calendar", all_failed);
        assert!(state.engine_sealed(), "{:?}", state.unsealed_reasons());
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
