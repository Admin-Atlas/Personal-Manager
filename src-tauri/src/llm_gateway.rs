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

use tauri::{AppHandle, Manager};

use crate::commands::{
    effective_models, BACKGROUND_AUTO_SWITCH_KEY, BACKGROUND_MODELS_KEY, CHAT_AUTO_SWITCH_KEY,
    CHAT_MODELS_KEY,
};
use crate::error::Result;
use crate::openrouter::{self, ChatMessage, Completion};
use crate::secret::Secret;
use crate::{secrets, AppState};

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

/// Runtime signals that can influence routing at dispatch time. **Deliberately inert today** — it
/// holds no fields. It is threaded through EVERY dispatch path (built inside [`resolve`], never by a
/// caller) so the power-aware provider policy (#432) can add battery / AC state here and change
/// routing in ONE place, without touching a single call site. Do NOT delete it or "simplify" it
/// away because it is currently empty: the emptiness IS the banked seam, and constructing it inside
/// `resolve` is precisely what keeps a future field from rippling out to the 13 dispatch sites.
#[derive(Clone, Copy, Debug, Default)]
pub struct RuntimeContext {}

impl RuntimeContext {
    /// Read the current runtime signals. Empty today; #432 populates it (battery / AC / power-saver
    /// state) at the I/O edge, consistent with the fit-math vs hardware-scan split.
    fn current() -> Self {
        Self {}
    }
}

/// The effective provider routing for a request, after [`resolve_provider`] applies runtime policy
/// to the raw preference. Today it mirrors the preference 1:1; #432 is where an empty-context
/// identity becomes a real policy (e.g. force `Cloud` on battery).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderChoice {
    Cloud,
    Local,
    LocalThenCloud,
}

/// Decide a role's effective provider from its preference and the current runtime context. **Pure**
/// — no I/O — so it is exhaustively unit-tested and the byte-identical invariant is mechanical, not
/// argued. `runtime` is intentionally unread TODAY: the power-aware policy (#432) will read
/// battery / AC state from it HERE to override the preference, which is why it is a parameter of
/// this one function rather than something the call sites compute. Keeping it in the signature is
/// what makes that feature a change to this function alone. Do not remove it.
pub fn resolve_provider(
    role: Role,
    prefs: &RoutingPrefs,
    runtime: &RuntimeContext,
) -> ProviderChoice {
    let _ = runtime; // reserved for #432 — see the doc comment; not a dead parameter.
    match prefs.for_role(role) {
        ProviderPref::Cloud => ProviderChoice::Cloud,
        ProviderPref::Local => ProviderChoice::Local,
        ProviderPref::LocalThenCloud => ProviderChoice::LocalThenCloud,
    }
}

/// The cloud arm's hydrated inputs: the API key and the ordered model list (auto-switch fallback).
pub struct CloudArm {
    pub key: Secret,
    pub models: Vec<String>,
}

/// The resolved route for a request — which provider to use, hydrated with what it needs. Today the
/// only inhabitant is `Cloud`; the local arms (`LocalOnly`, `LocalThenCloud`) land with the live
/// provider in #297's next PR, when a local endpoint can actually be configured.
pub enum RoutePlan {
    Cloud(CloudArm),
}

impl RoutePlan {
    /// The primary (first) model id this route will try — used to attribute logged spend when the
    /// server did not report which model actually served the request.
    pub fn primary_model_id(&self) -> &str {
        match self {
            RoutePlan::Cloud(arm) => arm.models.first().map(String::as_str).unwrap_or_default(),
        }
    }

    /// The ordered model list this route will use — for the cost logger, which prices per model.
    pub fn models(&self) -> &[String] {
        match self {
            RoutePlan::Cloud(arm) => &arm.models,
        }
    }
}

/// Settings keys for the per-role routing preference. Absent until #297's live provider writes them;
/// the reader below defaults an absent key to `Cloud`, which is what makes the seam strictly additive.
pub(crate) const CHAT_ROUTING_KEY: &str = "local_llm_chat_routing";
pub(crate) const BACKGROUND_ROUTING_KEY: &str = "local_llm_background_routing";

/// Parse a stored routing preference. Absent (every install today), `"cloud"`, or an unrecognised
/// value all resolve to `Cloud` — the strictly-additive default.
fn parse_pref(raw: Option<String>) -> ProviderPref {
    match raw.as_deref() {
        Some("local") => ProviderPref::Local,
        Some("local-then-cloud") => ProviderPref::LocalThenCloud,
        _ => ProviderPref::Cloud,
    }
}

fn routing_prefs(conn: &rusqlite::Connection) -> Result<RoutingPrefs> {
    Ok(RoutingPrefs {
        chat: parse_pref(crate::db::get_setting(conn, CHAT_ROUTING_KEY)?),
        background: parse_pref(crate::db::get_setting(conn, BACKGROUND_ROUTING_KEY)?),
    })
}

/// Resolve the route for a role: read the routing preference + runtime context, decide the provider
/// (pure), and hydrate the chosen arm. Returns `None` when no provider is usable (today: no API key
/// — a local endpoint cannot yet be configured), so each caller keeps its own no-provider behaviour
/// (a background job skips; an interactive command returns a friendly error). Takes only `role`; the
/// [`RuntimeContext`] is built here, never by the caller, so adding a future input never re-plumbs a
/// single dispatch site.
pub fn resolve(app: &AppHandle, role: Role) -> Result<Option<RoutePlan>> {
    let state = app.state::<AppState>();

    // Decide the provider (pure) under a short lock, dropped before the key read and the caller's own
    // DB work below — the DB mutex is non-reentrant, so nothing may hold it across `resolve`'s return.
    let choice = {
        let conn = state.conn()?;
        let prefs = routing_prefs(&conn)?;
        resolve_provider(role, &prefs, &RuntimeContext::current())
    };
    match choice {
        // Today every reachable choice hydrates to the cloud arm: `Cloud` directly, and
        // `LocalThenCloud` degrades to cloud because no local endpoint exists yet. Both fall through.
        ProviderChoice::Cloud | ProviderChoice::LocalThenCloud => {}
        // Local-only has no configurable endpoint until #297's live provider, so nothing can serve
        // it — treat as "no provider" (the caller's no-provider branch). Unreachable in practice
        // today because no install can set a local preference yet.
        ProviderChoice::Local => return Ok(None),
    }

    // The role's key: chat uses the primary key; background prefers the dedicated background key and
    // falls back to the primary — exactly as the call sites did before this seam.
    let key = match role {
        Role::Chat => secrets::get_openrouter_key()?,
        Role::Background => secrets::get_background_or_primary_key()?,
    };
    let Some(key) = key else {
        return Ok(None);
    };
    let (models_key, auto_key) = match role {
        Role::Chat => (CHAT_MODELS_KEY, CHAT_AUTO_SWITCH_KEY),
        Role::Background => (BACKGROUND_MODELS_KEY, BACKGROUND_AUTO_SWITCH_KEY),
    };
    let models = {
        let conn = state.conn()?;
        effective_models(&conn, models_key, auto_key)?
    };
    Ok(Some(RoutePlan::Cloud(CloudArm { key, models })))
}

/// Run a non-streaming completion through the resolved route. `app` is reserved for the local-slot +
/// cooldown machinery that lands with the live provider (#297's next PR); only the cloud arm exists
/// here, so it is unused today.
pub async fn complete(
    app: &AppHandle,
    plan: &RoutePlan,
    messages: &[ChatMessage],
    cache_prefix: bool,
) -> Result<Completion> {
    let _ = app; // reserved for the local slot/cooldown (#297) — see the doc comment.
    match plan {
        RoutePlan::Cloud(arm) => {
            openrouter::complete(arm.key.expose(), &arm.models, messages, cache_prefix).await
        }
    }
}

/// Stream a chat completion through the resolved route, forwarding each token to `on_token`. `app`
/// is reserved for the local slot/cooldown (#297's next PR); only the cloud arm exists here.
pub async fn stream_chat<F>(
    app: &AppHandle,
    plan: &RoutePlan,
    messages: &[ChatMessage],
    cache_through: Option<usize>,
    on_token: F,
) -> Result<Completion>
where
    F: FnMut(&str),
{
    let _ = app; // reserved for the local slot/cooldown (#297) — see the doc comment.
    match plan {
        RoutePlan::Cloud(arm) => {
            openrouter::stream_chat(
                arm.key.expose(),
                &arm.models,
                messages,
                cache_through,
                on_token,
            )
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The byte-identical invariant made mechanical: with an EMPTY runtime context, the resolver's
    /// output must equal the raw preference for every (role, preference) pair — there is no policy
    /// override yet, so routing is exactly a direct preference lookup. The same reasoning as making
    /// the IPC boundary enforced rather than merely documented (#432 items 1-3).
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
}
