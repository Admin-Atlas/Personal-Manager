// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The Local AI switch (Mods spec §2, decision D7): one stored setting that turns the whole Local
//! AI Workbench on or off. It is enforced here in Rust, not only by hiding the tab, because what it
//! promises is about behaviour: with it off, nothing local runs and nothing that was routed to a
//! local model quietly moves to the cloud.
//!
//! Switching it OFF follows Bobby's rule word for word, "just empty it and stop ai and tell them to
//! pick a model":
//! - **empty it** — both chosen local models are deleted. The server address and token stay, so
//!   switching back on is one click, and routing stays too, which is what makes the next point hold.
//! - **stop ai** — every role routed to a local model ("Local only" and "Local, fall back to cloud"
//!   alike) resolves to nothing while off ([`crate::llm_gateway::resolve_provider`]). That is why
//!   neither existing forget path could be reused: `clear_local_llm_endpoint` leaves routing in
//!   place, so a local-then-cloud role with no local model falls through to the cloud, and
//!   `reset_local_ai_settings` deletes routing, which reads back as Cloud. A running download, a
//!   test and any background call stop where they stand, and what PM loaded on the graphics card is
//!   handed back. The release scheduler and the 30-second power watcher park until it is on again.
//! - **tell them to pick a model** — a role with nothing to answer it gets [`LOCAL_AI_OFF_MESSAGE`],
//!   and AI & Models shows the same words beside a cloud model picker for each such role.
//!
//! Switching back ON reconnects the server, and nothing else: the models it emptied are still
//! gone, so each role it emptied stays in the "pick a model" state (D7) until a model or a routing
//! is chosen for it, or the server is forgotten ([`crate::llm_gateway::Emptied`]). Without that
//! record, "Local, fall back to cloud" with no model would fall straight through to the cloud the
//! moment the switch came back.
//!
//! Defaults (D7): a brand-new store starts off; onboarding's "on this device" choice switches it on;
//! a store that already holds any Local AI setting (`local_ai::local_ai_settings_stored`) starts
//! on, so nobody's local setup stops working on upgrade. The default is decided once, at the first
//! open of each store ([`stamp_default`]), and written down, because the rule it is decided by
//! reads rows that later actions write: the cloud picker stores a routing row, and Forget server
//! deletes some, either of which would otherwise flip the switch behind the user's back.

use rusqlite::Connection;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::error::{Error, Result};
use crate::llm_gateway::{
    self, ProviderChoice, ProviderPref, Role, RoutingPrefs, RuntimeContext, LOCAL_BASE_URL_KEY,
};
use crate::{db, residency, secrets, AppState};

/// The stored switch, `"true"` or `"false"`. Absent until the store's first open decides it.
///
/// Not one of `local_ai::LOCAL_AI_SETTINGS`: it is the tab's master, not a setting on it, so the
/// tab's "Reset to defaults" leaves it alone (resetting the tab must not make the tab vanish), and
/// counting it there would make the default it is derived from depend on itself.
pub(crate) const LOCAL_AI_ENABLED_KEY: &str = "local_ai_enabled";

/// What a role routed to a local model is told while Local AI is off — by the commands that need a
/// model, the executor's gate and AI & Models alike, in Bobby's words.
pub(crate) const LOCAL_AI_OFF_MESSAGE: &str =
    "Local AI is off. Pick a model in AI & Models to keep using AI.";

/// What such a role is told once Local AI is back on and no model has been picked for it yet: the
/// same instruction, pointed at the tab where local models are chosen.
pub(crate) const PICK_A_MODEL_MESSAGE: &str =
    "Local AI has no model picked for this. Pick one in Settings → Local AI to keep using AI.";

/// The refusal a command that does local work gives while the switch is off. Reached only by a
/// stale view or a script: the controls that call these are gone with the tab.
const OFF_REFUSAL: &str = "Local AI is off. Turn it on in Settings → AI & Models first.";

/// A stored row's meaning: `None` for an absent row or one PM never writes.
fn parse(raw: Option<&str>) -> Option<bool> {
    match raw {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    }
}

/// The switch as the open store has it. A store that has not been opened since this shipped has no
/// row yet, and reads as D7's default without writing it, so this is safe on any guard.
pub(crate) fn read(conn: &Connection) -> Result<bool> {
    match parse(db::get_setting(conn, LOCAL_AI_ENABLED_KEY)?.as_deref()) {
        Some(on) => Ok(on),
        None => crate::local_ai::local_ai_settings_stored(conn),
    }
}

/// Decide D7's default for a store that has no row yet, and write it down. Returns the switch. Run
/// on every store open (`AppState::open_session`); a store that already has a row is only read.
pub(crate) fn stamp_default(conn: &Connection) -> Result<bool> {
    if let Some(on) = parse(db::get_setting(conn, LOCAL_AI_ENABLED_KEY)?.as_deref()) {
        return Ok(on);
    }
    let on = crate::local_ai::local_ai_settings_stored(conn)?;
    db::set_bool(conn, LOCAL_AI_ENABLED_KEY, on)?;
    Ok(on)
}

/// The store half of switching off, in one transaction so it is never half done: the switch, both
/// chosen local models, a record of each role that loses its model, and onboarding marked done.
/// The last because a keyless local user who predates that flag was kept out of the first-run
/// wizard only by their local model, and the wizard is not how anyone should be told to pick
/// another one.
///
/// The record ([`llm_gateway::Emptied`]) is kept only for a role routed to a local model that had
/// one: that is a role this empties, and it must wait for a pick once the switch is back on. A role
/// routed "Local, fall back to cloud" that never had a model went to the cloud before switching
/// off, and goes there again after.
///
/// What it leaves is as deliberate: the server address and its token (switching back on is one
/// click), routing (a local-routed role must keep reading as local, so it resolves to nothing
/// rather than to the cloud), and every other Local AI setting.
pub(crate) fn write_off(conn: &Connection) -> Result<()> {
    let tx = conn.unchecked_transaction()?;
    let prefs = llm_gateway::routing_prefs(&tx)?;
    for role in [Role::Chat, Role::Background] {
        let (model_key, emptied_key) = llm_gateway::local_model_keys(role);
        let had_model = db::get_setting(&tx, model_key)?.is_some_and(|m| !m.trim().is_empty());
        if had_model && prefs.for_role(role) != ProviderPref::Cloud {
            db::set_bool(&tx, emptied_key, true)?;
        }
        db::delete_setting(&tx, model_key)?;
    }
    db::set_bool(&tx, LOCAL_AI_ENABLED_KEY, false)?;
    db::set_bool(&tx, crate::settings::ONBOARDING_DONE_KEY, true)?;
    tx.commit()?;
    Ok(())
}

/// Refuse a command that does local work (a probe, a download, a test, a scan) while the switch is
/// off. Reads the runtime copy, which every store open and every switch keeps in step with the row.
pub(crate) fn require_on(app: &AppHandle) -> Result<()> {
    if app.state::<AppState>().local_ai.local_ai_on() {
        Ok(())
    } else {
        Err(Error::Other(OFF_REFUSAL.into()))
    }
}

/// What AI & Models needs to draw the switch's row and its "pick a model" pickers.
#[derive(Serialize, Debug, PartialEq, Eq)]
pub struct LocalAiSwitch {
    pub on: bool,
    /// Chat is routed to a local model while Local AI is off, so nothing answers it until a model is
    /// picked. Always false while on.
    pub chat_needs_model: bool,
    pub background_needs_model: bool,
    /// A server address or a token is still saved, so "Forget server and token" has something to
    /// forget.
    pub server_saved: bool,
}

/// Pure, so the rule is tested. "Needs a model" is exactly the routing rule's answer, asked of the
/// routing rule itself, so the pickers can never offer a fix for a role the gateway would serve.
fn switch_view(prefs: &RoutingPrefs, server_saved: bool) -> LocalAiSwitch {
    let needs = |role| {
        llm_gateway::resolve_provider(role, prefs, &RuntimeContext::default())
            == ProviderChoice::LocalAiOff
    };
    LocalAiSwitch {
        on: prefs.local_ai_on,
        chat_needs_model: needs(Role::Chat),
        background_needs_model: needs(Role::Background),
        server_saved,
    }
}

/// Read the view. The DB guard closes before the keychain read, which can wait on a consent prompt.
fn current(state: &AppState) -> Result<LocalAiSwitch> {
    let (prefs, base_url_saved) = {
        let conn = state.conn()?;
        (
            llm_gateway::routing_prefs(&conn)?,
            db::get_setting(&conn, LOCAL_BASE_URL_KEY)?.is_some(),
        )
    };
    // A keychain PM can't read reads as "a token may be there": hiding the one control that
    // forgets it because PM couldn't look would be the wrong way round.
    let token_saved = secrets::has_local_llm_endpoint_token().unwrap_or(true);
    Ok(switch_view(&prefs, base_url_saved || token_saved))
}

/// The switch, for AI & Models, the Settings rail and the Local AI tab.
#[tauri::command]
pub fn get_local_ai_switch(state: State<'_, AppState>) -> Result<LocalAiSwitch> {
    current(&state)
}

/// Switch Local AI on or off, and answer with where that leaves things.
///
/// On writes the row and wakes the two loops; nothing else changes, because everything off left
/// alone (the server, the token, routing, every policy) is exactly what on needs. The models off
/// emptied are not put back: each such role waits for a pick in the Local AI tab, never the cloud.
///
/// Off runs in an order chosen so that it fails closed. The runtime copy goes first, because it is
/// what stops work already under way: the executor's gate and the background call's race read it,
/// so from this instant nothing local starts and nothing falls back to the cloud. Then the store
/// (one transaction, [`write_off`]); if that fails, the runtime copy is put back, so the two never
/// disagree, and nothing else has been touched. Then the running download and test are stopped
/// (both race the switch going off, so they stop on their own; the pull is also told directly, so
/// its HTTP request is dropped at once), and what PM loaded is handed back in the background: an
/// unload waits for the slot, and a chat reply already streaming finishes first, so the command
/// doesn't wait for it.
#[tauri::command]
pub fn set_local_ai_enabled(app: AppHandle, on: bool) -> Result<LocalAiSwitch> {
    let state = app.state::<AppState>();
    if on {
        {
            let conn = state.conn()?;
            db::set_bool(&conn, LOCAL_AI_ENABLED_KEY, true)?;
        }
        state.local_ai.set_local_ai_on(true);
        llm_gateway::ping_status(&app);
        return current(&state);
    }

    let was_on = state.local_ai.local_ai_on();
    state.local_ai.set_local_ai_on(false);
    let written = state.conn().and_then(|conn| write_off(&conn));
    if let Err(e) = written {
        state.local_ai.set_local_ai_on(was_on);
        return Err(e);
    }
    state.local_ai.cancel_pull();
    // A finished test proved a model answered that is no longer chosen.
    state.local_ai.clear_finished_test();
    spawn_release(&app);
    // The footer, the chips and the On battery strip all read the status, which now says nothing
    // local is configured.
    llm_gateway::ping_status(&app);
    current(&state)
}

/// Hand back everything PM loaded, wherever it loaded it (like the exit hook, not filtered to the
/// configured server: a model PM put on an old one is still PM's to free), and regardless of the
/// release policy: switching off is somebody asking, as the "Release now" button is. The token goes
/// only to the server it was saved for (`residency::token_for`).
fn spawn_release(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let configured = state
            .conn()
            .ok()
            .and_then(|conn| db::get_setting(&conn, LOCAL_BASE_URL_KEY).ok().flatten())
            .or_else(|| state.local_ai.cached_release_endpoint());
        let endpoints = residency::release_endpoints(None, &state.local_ai.pm_loaded_pairs());
        if endpoints.is_empty() {
            return;
        }
        let token = secrets::get_local_llm_endpoint_token()
            .ok()
            .flatten()
            .map(|s| s.expose().to_string());
        let mut changed = false;
        for base_url in endpoints {
            if state.local_ai.has_no_unload_route(&base_url) {
                continue;
            }
            let token = residency::token_for(&base_url, configured.as_deref(), token.as_deref());
            let pass = crate::local_ai::release_pm_models(&state, &base_url, token).await;
            changed |= pass.freed > 0 || pass.unconfirmed > 0 || pass.no_route;
        }
        if changed {
            llm_gateway::ping_status(&app);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::llm_gateway::{
        Emptied, BACKGROUND_ROUTING_KEY, CHAT_ROUTING_KEY, LOCAL_BACKGROUND_MODEL_KEY,
        LOCAL_CHAT_MODEL_KEY,
    };

    fn mem_conn() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE settings(key TEXT PRIMARY KEY, value TEXT)",
            [],
        )
        .unwrap();
        conn
    }

    /// D7's three defaults: a brand-new store starts off, a store with any Local AI setting starts
    /// on, and once decided the answer is written down and no longer follows the rows it came from.
    #[test]
    fn the_default_is_decided_once_from_what_the_store_already_holds() {
        let fresh = mem_conn();
        assert!(!read(&fresh).unwrap(), "a brand-new store reads as off");
        assert!(!stamp_default(&fresh).unwrap());
        assert_eq!(
            db::get_setting(&fresh, LOCAL_AI_ENABLED_KEY)
                .unwrap()
                .as_deref(),
            Some("false"),
            "and the answer is written down"
        );
        // The cloud picker writing a routing row must not switch it on afterwards.
        db::set_setting(&fresh, CHAT_ROUTING_KEY, "cloud").unwrap();
        assert!(!read(&fresh).unwrap());
        assert!(!stamp_default(&fresh).unwrap());

        let existing = mem_conn();
        db::set_setting(&existing, LOCAL_BASE_URL_KEY, "http://127.0.0.1:11434").unwrap();
        assert!(
            read(&existing).unwrap(),
            "an existing local setup reads as on"
        );
        assert!(stamp_default(&existing).unwrap());
        // Forgetting the server later must not switch it off behind the user's back.
        db::delete_setting(&existing, LOCAL_BASE_URL_KEY).unwrap();
        assert!(read(&existing).unwrap());
        assert!(stamp_default(&existing).unwrap());
    }

    #[test]
    fn a_row_pm_never_writes_reads_as_the_default() {
        let conn = mem_conn();
        db::set_setting(&conn, LOCAL_AI_ENABLED_KEY, "yes").unwrap();
        assert!(!read(&conn).unwrap());
        db::set_setting(&conn, LOCAL_CHAT_MODEL_KEY, "gemma3:4b").unwrap();
        assert!(read(&conn).unwrap());
    }

    /// "Just empty it": the chosen models go, and the server, the token's place, routing and
    /// every other setting stay, so the routing rule then sends every local role to nothing.
    #[test]
    fn switching_off_empties_the_models_and_keeps_the_server_and_routing() {
        let conn = mem_conn();
        db::set_setting(&conn, LOCAL_BASE_URL_KEY, "http://127.0.0.1:11434").unwrap();
        db::set_setting(&conn, LOCAL_CHAT_MODEL_KEY, "gemma3:4b").unwrap();
        db::set_setting(&conn, LOCAL_BACKGROUND_MODEL_KEY, "qwen3:8b").unwrap();
        db::set_setting(&conn, CHAT_ROUTING_KEY, "local-then-cloud").unwrap();
        db::set_setting(&conn, BACKGROUND_ROUTING_KEY, "local").unwrap();
        db::set_setting(&conn, residency::RELEASE_POLICY_KEY, "idle").unwrap();
        assert!(stamp_default(&conn).unwrap());

        write_off(&conn).unwrap();

        assert!(!read(&conn).unwrap());
        assert_eq!(db::get_setting(&conn, LOCAL_CHAT_MODEL_KEY).unwrap(), None);
        assert_eq!(
            db::get_setting(&conn, LOCAL_BACKGROUND_MODEL_KEY).unwrap(),
            None
        );
        assert_eq!(
            db::get_setting(&conn, LOCAL_BASE_URL_KEY)
                .unwrap()
                .as_deref(),
            Some("http://127.0.0.1:11434"),
            "the server is kept, so switching back on is one click"
        );
        assert_eq!(
            db::get_setting(&conn, CHAT_ROUTING_KEY).unwrap().as_deref(),
            Some("local-then-cloud")
        );
        assert_eq!(
            db::get_setting(&conn, residency::RELEASE_POLICY_KEY)
                .unwrap()
                .as_deref(),
            Some("idle")
        );
        assert!(db::get_bool(&conn, crate::settings::ONBOARDING_DONE_KEY, false).unwrap());

        let prefs = llm_gateway::routing_prefs(&conn).unwrap();
        for role in [Role::Chat, Role::Background] {
            assert_eq!(
                llm_gateway::resolve_provider(role, &prefs, &RuntimeContext::default()),
                ProviderChoice::LocalAiOff,
                "{role:?} must have nothing to answer it, never the cloud"
            );
        }
        assert_eq!(
            prefs.emptied,
            Emptied {
                chat: true,
                background: true,
            },
            "both roles lost a model they were routed to"
        );
        assert!(
            !llm_gateway::local_chat_configured(&conn).unwrap(),
            "off is never a ready local provider"
        );
    }

    /// Back on, "turning it back on is one click" reconnects the server and no more: a role it
    /// emptied waits for a model and never falls through to the cloud. A role that had no model to
    /// empty (local-then-cloud before one was picked, as on main) keeps going to the cloud, and a
    /// cloud role is left out of it.
    #[test]
    fn back_on_an_emptied_role_waits_for_a_pick_and_the_others_route_as_before() {
        let conn = mem_conn();
        db::set_setting(&conn, LOCAL_BASE_URL_KEY, "http://127.0.0.1:11434").unwrap();
        db::set_setting(&conn, CHAT_ROUTING_KEY, "local-then-cloud").unwrap();
        db::set_setting(&conn, LOCAL_CHAT_MODEL_KEY, "gemma3:4b").unwrap();
        db::set_setting(&conn, BACKGROUND_ROUTING_KEY, "local-then-cloud").unwrap();
        assert!(stamp_default(&conn).unwrap());

        write_off(&conn).unwrap();
        // What `set_local_ai_enabled(on = true)` writes.
        db::set_bool(&conn, LOCAL_AI_ENABLED_KEY, true).unwrap();

        let prefs = llm_gateway::routing_prefs(&conn).unwrap();
        assert!(prefs.local_ai_on);
        assert_eq!(
            prefs.emptied,
            Emptied {
                chat: true,
                background: false,
            }
        );
        let ctx = RuntimeContext::default();
        assert_eq!(
            llm_gateway::resolve_provider(Role::Chat, &prefs, &ctx),
            ProviderChoice::Local,
            "served as Local only: with no model, nothing answers"
        );
        assert_eq!(
            llm_gateway::no_provider_message_with(&prefs, Role::Chat),
            PICK_A_MODEL_MESSAGE
        );
        assert_eq!(
            llm_gateway::resolve_provider(Role::Background, &prefs, &ctx),
            ProviderChoice::LocalThenCloud,
            "never had a model, so it falls to the cloud as it did before"
        );
        // On, the AI & Models pickers stay away: picking a local model is the Local AI tab's.
        let view = switch_view(&prefs, true);
        assert!(view.on && !view.chat_needs_model && !view.background_needs_model);

        // Off again with nothing chosen since: still waiting, and nothing new recorded.
        write_off(&conn).unwrap();
        assert_eq!(
            llm_gateway::routing_prefs(&conn).unwrap().emptied,
            Emptied {
                chat: true,
                background: false,
            }
        );
    }

    #[test]
    fn the_view_offers_a_pick_only_for_a_local_role_while_off() {
        let prefs = |chat, background, on| RoutingPrefs {
            chat,
            background,
            local_ai_on: on,
            emptied: llm_gateway::Emptied::default(),
        };
        use ProviderPref::*;
        assert_eq!(
            switch_view(&prefs(LocalThenCloud, Cloud, false), true),
            LocalAiSwitch {
                on: false,
                chat_needs_model: true,
                background_needs_model: false,
                server_saved: true,
            }
        );
        assert_eq!(
            switch_view(&prefs(Cloud, Local, false), false),
            LocalAiSwitch {
                on: false,
                chat_needs_model: false,
                background_needs_model: true,
                server_saved: false,
            }
        );
        // On, nothing needs a pick: routing has its home in the Local AI tab.
        for pref in [Cloud, Local, LocalThenCloud] {
            let v = switch_view(&prefs(pref, pref, true), true);
            assert!(v.on && !v.chat_needs_model && !v.background_needs_model);
        }
    }
}
