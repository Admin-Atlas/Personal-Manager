// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The calendar mirror: Google, Outlook and iCal accounts, their calendars, and the shared sync
//! pass over every provider. Read-only, except that a Google account can have editing turned on
//! ([`enable_calendar_editing`], #884).
//!
//! `set_google_client` / `clear_google_client` live here because that is where they sit in
//! the Google Calendar flow and `clear_google_client` reads calendar rows — but the BYO
//! OAuth client they manage is ONE client serving Calendar *and* Drive, so a change here
//! reaches `connectors` and `backup::schedule` too.

use std::collections::BTreeMap;

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::calendar::{self, CalendarEvent, IcsFeedInfo};
use crate::calendar_editing::{self, Choice, ConsentVerdict, EditingStatus};
use crate::calendar_write::dto::ReadOnlyReason;
use crate::calendar_write::reconcile::Stamp;
use crate::error::{Error, Result};
use crate::google;
use crate::{briefing, drive, flags, microsoft, outlook_calendar, secrets, AppState};

use super::shared::own_client;
use super::shared::resolve_zone;
use super::shared::{
    google_account_emails, google_grant_token_keys, google_grant_users, pin_existing_grants,
    release_plan, require_main_window, saved_project_account, wrong_account, GoogleDisconnect,
    GoogleUse,
};
use super::vaults::require_vault_owner;

// --- personal assistant: calendar (multi-provider, read-only — cards 6A/6B) ---
//
// The calendar surface is multi-PROVIDER and multi-ACCOUNT: Google (OAuth, per-account), Outlook
// (Microsoft Graph OAuth, per-account), and Apple/any iCal subscription all flow into one normalised
// account → calendar → event model (see `crate::calendar`). The new `calendar_overview`,
// per-provider connect/disconnect, and `set_calendar_selected` commands drive it; the older
// single-account commands further down are thin back-compat wrappers over the same model, kept
// working until the Settings UI is rewired (PR2).

/// The per-account Google Calendar keychain token key (`google_oauth_token_calendar::<email>`).
fn google_calendar_token_key(email: &str) -> String {
    secrets::token_key_for("google", "calendar", email)
        .expect("google/calendar is a token-bearing pair")
}

/// Everything the Connectors → Calendar UI needs in one read: which provider clients are configured,
/// every connected account/subscription, and every registered calendar (with its selection).
#[derive(Serialize)]
pub struct CalendarOverview {
    pub google_client_configured: bool,
    pub microsoft_client_configured: bool,
    pub accounts: Vec<calendar::CalendarAccount>,
    pub calendars: Vec<calendar::Calendar>,
    pub last_sync: Option<String>,
    pub window_days: i64,
    /// The mirrored band `[start, end]` (RFC3339, from [`calendar::time_window`]) — so the unified
    /// view can tell when the user has paged past the synced range and show an "outside synced
    /// range" hint rather than a misleadingly-empty grid.
    pub mirror_start: String,
    pub mirror_end: String,
    /// Whether PM may edit each Google account's calendars, keyed by account id (`gcal:<email>`).
    /// Computed here only, from the stored choice and the account's calendar token.
    pub editing: BTreeMap<String, EditingStatus>,
}

/// The unified calendar state across every provider. Runs the one-time legacy Google migration first
/// so an upgrading single-account user appears in the new model.
#[tauri::command]
pub async fn calendar_overview(app: AppHandle) -> Result<CalendarOverview> {
    let _ = migrate_legacy_google_calendar(&app).await;
    let state = app.state::<AppState>();
    let (accounts, calendars, last_sync, (mirror_start, mirror_end), choices) = {
        let conn = state.conn()?;
        (
            calendar::list_sources(&conn, None)?,
            calendar::list_calendars(&conn)?,
            calendar::last_sync(&conn)?,
            calendar::time_window(&conn)?,
            calendar_editing::choices(&conn)?,
        )
    };
    // Token scopes come from the keychain, read with the DB lock released. Only an account with a
    // stored choice needs one: without it, editing is off whatever the token holds.
    let editing = accounts
        .iter()
        .filter(|a| a.provider == "google")
        .filter_map(|a| {
            let email = a.email.as_deref()?;
            let choice = calendar_editing::lookup(&choices, email);
            let scope = choice
                .and_then(|_| google::token_scope(&google_calendar_token_key(email)).ok())
                .flatten();
            Some((
                a.id.clone(),
                calendar_editing::status(choice, scope.as_deref()),
            ))
        })
        .collect();
    Ok(CalendarOverview {
        google_client_configured: google::has_client()?,
        microsoft_client_configured: microsoft::has_client()?,
        accounts,
        calendars,
        last_sync,
        window_days: calendar::AGENDA_DAYS,
        mirror_start,
        mirror_end,
        editing,
    })
}

/// Tick/untick one calendar (by its `calendars.id`) for syncing.
#[tauri::command]
pub fn set_calendar_selected(
    state: State<'_, AppState>,
    calendar_id: String,
    selected: bool,
) -> Result<()> {
    let conn = state.conn()?;
    calendar::set_calendar_selected(&conn, &calendar_id, selected)
}

/// Type one calendar as work or personal, or clear it with `None` (v45).
///
/// Per-calendar rather than per-event because the user has already drawn that line by connecting the
/// accounts separately. Nothing consumes the typing yet — the Work-context score and the
/// person-context flags are its first readers.
#[tauri::command]
pub fn set_calendar_kind(
    state: State<'_, AppState>,
    calendar_id: String,
    kind: Option<String>,
) -> Result<()> {
    let conn = state.conn()?;
    calendar::set_calendar_kind(&conn, &calendar_id, kind.as_deref())
}

/// Mark one calendar (by its `calendars.id`) quiet, or not: keep it on the Calendar tab but exclude
/// its events from the assistant (briefing, flags/reminders, chat agenda, focus upcoming).
/// No re-sync needed — the events stay mirrored; only the assistant query path filters them.
#[tauri::command]
pub fn set_calendar_quiet(
    state: State<'_, AppState>,
    calendar_id: String,
    quiet: bool,
) -> Result<()> {
    let conn = state.conn()?;
    calendar::set_calendar_quiet(&conn, &calendar_id, quiet)
}

// --- Google Calendar (OAuth, per-account) ---

/// The core connect flow: run consent, learn the account from its primary calendar (id == email),
/// store the token under that account's key, and register the account + its calendars (all selected
/// by default). The sign-in uses `own` (a project pasted for this account), else the client already
/// saved for `saved_account` (an Advanced-Protection account connected to another Google service),
/// else the shared client.
async fn do_connect_google_calendar(
    app: &AppHandle,
    own: Option<(String, String)>,
    saved_account: Option<String>,
) -> Result<calendar::CalendarAccount> {
    let state = app.state::<AppState>();
    let token = match (&own, &saved_account) {
        (Some((id, secret)), _) => {
            google::run_consent_with_client(
                google::CALENDAR_SCOPE,
                "Google Calendar",
                id.clone(),
                secret.clone(),
            )
            .await?
        }
        (None, Some(email)) => {
            google::run_consent_with_saved_project(email, google::CALENDAR_SCOPE, "Google Calendar")
                .await?
        }
        (None, None) => google::run_consent(google::CALENDAR_SCOPE, "Google Calendar").await?,
    };
    let raw = calendar::fetch_calendar_list_with_token(&token).await?;
    let email = raw
        .iter()
        .find(|c| c.primary)
        .map(|c| c.id.clone())
        .ok_or_else(|| {
            Error::Other("Google didn't return a primary calendar to identify the account.".into())
        })?;
    // Normalise the account identity (trim + lowercase) so a reconnect that returns a
    // differently-cased address updates the same source/token instead of duplicating it.
    let email = email.trim().to_lowercase();
    if let Some(expected) = &saved_account {
        if !expected.eq_ignore_ascii_case(&email) {
            return Err(wrong_account(expected, &email));
        }
    }
    let account = calendar::google_account_id(&email);
    if let Some((id, secret)) = &own {
        let keys = google_grant_token_keys(&*state.conn()?, &email)?;
        pin_existing_grants(&keys).await?;
        secrets::set_google_client_for_account(&email, id, secret)?;
    }
    google::save_consented_token(&google_calendar_token_key(&email), &token).await?;
    let conn = state.conn()?;
    calendar::upsert_source(&conn, &account, "google", Some(&email), &email)?;
    // A read-only connect asked Google for reading only, so it turns editing off even when the
    // account had it: only the "Turn on editing" consent sets the choice.
    calendar_editing::set_choice(&conn, &email, None)?;
    let inputs: Vec<_> = raw.iter().map(|c| c.to_input()).collect();
    // Connect UPSERTS the (in-hand, single-page) list but never prunes: a reconnect must not delete
    // page-two calendars a prior full sync registered. The first `sync_calendar` reconcile prunes off
    // a proper paginated, complete list.
    calendar::register_calendars(&conn, &account, "google", &inputs, false, |_| true)?;
    calendar::list_sources(&conn, Some("google"))?
        .into_iter()
        .find(|a| a.id == account)
        .ok_or_else(|| Error::Other("the account registration could not be read back".into()))
}

/// Connect a Google Calendar account (multi-account). Optionally signs in with the account's OWN
/// Cloud project (`client_id`/`client_secret`) — the Advanced-Protection path, mirroring `connect_drive`
/// — or with the project already saved for `account` (from [`google_saved_projects`]).
#[tauri::command]
pub async fn connect_google_calendar_account(
    app: AppHandle,
    client_id: Option<String>,
    client_secret: Option<String>,
    account: Option<String>,
) -> Result<calendar::CalendarAccount> {
    require_vault_owner(&app)?;
    let own = own_client(client_id, client_secret)?;
    do_connect_google_calendar(&app, own, saved_project_account(account)).await
}

/// Disconnect one Google Calendar account: drop its registry source (cascading its calendars +
/// mirrored events) and forget its token. The grant at Google and any per-account
/// (Advanced-Protection) client are released only when no other PM feature still uses the account
/// ([`release_plan`]); the result names the features that kept them.
#[tauri::command]
pub async fn disconnect_google_calendar_account(
    app: AppHandle,
    state: State<'_, AppState>,
    email: String,
) -> Result<GoogleDisconnect> {
    require_vault_owner(&app)?;
    // Read the account's users under a short lock, released before the revoke's await (rule #4).
    let plan = {
        let conn = state.conn()?;
        release_plan(&google_grant_users(&conn, &email)?, GoogleUse::Calendar)
    };
    // L-3: sever the grant at Google's end BEFORE forgetting the local token (best-effort, like
    // wipe) — but only when Calendar is the account's last user. Google revokes the whole grant, so
    // revoking here while Drive or backup still sign in as this account would cut them off too.
    if plan.revoke {
        if let Ok(Some(blob)) = secrets::get_google_token_for(&google_calendar_token_key(&email)) {
            let _ = google::revoke(blob.expose()).await;
        }
    }
    // The key's refresh lock, taken before the DB guard and held through the delete: a refresh already
    // in flight would otherwise save its token back after the delete, leaving a live sign-in (one that
    // may carry the calendar write scope) with no account left to disconnect it.
    let _refresh = crate::oauth_loopback::refresh_lock(&google_calendar_token_key(&email)).await;
    let conn = state.conn()?;
    // Clear the OAuth token FIRST and propagate a real failure (a locked keychain): dropping the DB
    // source before an un-clearable token would orphan the token with no source left to re-clear it.
    // `secrets::delete` treats a missing entry as success, so a returned Err is a genuine failure.
    secrets::clear_google_token_for(&google_calendar_token_key(&email))?;
    if plan.forget_account_client {
        // Per-AP client; absent for shared-client accounts. Kept while Drive or backup still
        // refresh through it. Also before the row, and fatal: when Calendar was the account's last
        // user, nothing but this row names the account, so a client left behind once it's gone
        // outlives even Remove PM data (#893).
        secrets::clear_google_client_for_account(&email)?;
    }
    calendar::remove_source(&conn, &calendar::google_account_id(&email))?;
    calendar_editing::set_choice(&conn, &email, None)?;
    // A grant Google keeps still carries the write scope if editing was ever granted; a revoke ends it.
    let calendar_write = !plan.revoke && calendar_editing::write_granted(&conn, &email)?;
    if plan.revoke {
        calendar_editing::forget_write_grant(&conn, &email)?;
    }
    Ok(GoogleDisconnect {
        kept_for: plan.kept_for,
        calendar_write,
    })
}

/// Turn on editing for one connected Google Calendar account (`email` as the account list spells it):
/// a consent for reading AND writing, stated in full, through the client that minted the account's
/// calendar token, with Google's chooser starting on that account. Classified before anything is
/// saved ([`calendar_editing::classify_consent`]): another account signing in, or the read box
/// unticked, changes nothing; the write box unticked keeps the fresh (read-only) token and leaves
/// editing off, which is the status returned.
#[tauri::command]
pub async fn enable_calendar_editing(
    app: AppHandle,
    window: tauri::Window,
    email: String,
) -> Result<EditingStatus> {
    require_main_window(&window)?;
    require_vault_owner(&app)?;
    let email = email.trim().to_string();
    let account = calendar::google_account_id(&email);
    let connected = |conn: &rusqlite::Connection| -> Result<bool> {
        Ok(calendar::list_sources(conn, Some("google"))?
            .iter()
            .any(|a| a.id == account))
    };
    let state = app.state::<AppState>();
    if !connected(&*state.conn()?)? {
        return Err(Error::Other(
            "Connect this Google Calendar account first.".into(),
        ));
    }
    let token_key = google_calendar_token_key(&email);
    let token = google::run_consent_for_key(
        &token_key,
        &email,
        &google::calendar_editing_scopes(),
        "Google Calendar editing",
    )
    .await?;
    let granted = token.scope.clone().unwrap_or_default();
    // Which account signed in is read through the calendar list, so only with the read scope.
    let signed_in_as = if google::scope_set_has(&granted, google::CALENDAR_SCOPE) {
        Some(calendar::fetch_primary_calendar_id_with_token(&token).await?)
    } else {
        None
    };
    let verdict = calendar_editing::classify_consent(&email, &granted, signed_in_as.as_deref());
    match verdict {
        ConsentVerdict::ReadonlyMissing => {
            return Err(Error::Other(
                "Google didn't give PM permission to see your calendars, so nothing changed. Try \
                 again and leave both boxes ticked: PM needs to see your calendars to keep them in \
                 sync, as well as to change events."
                    .into(),
            ))
        }
        ConsentVerdict::WrongAccount { signed_in_as } => {
            return Err(wrong_account(&email, &signed_in_as))
        }
        ConsentVerdict::Enabled | ConsentVerdict::WriteDeclined => {}
    }
    // Check, save and record as one step: the key's refresh lock (so a refresh in flight can't save the
    // old token over this one), then the DB guard, which a Disconnect or Clear client holds across its
    // own delete, so neither can land between the check and the save. Lock order matches
    // `disconnect_google_calendar_account`: refresh lock, then DB.
    let _refresh = crate::oauth_loopback::refresh_lock(&token_key).await;
    let conn = state.conn()?;
    // Disconnected while Google's page was open: saving now would leave a token no account owns.
    if !connected(&conn)? {
        return Err(Error::Other(
            "This account was disconnected while Google was asking, so PM didn't keep the sign-in."
                .into(),
        ));
    }
    google::save_token(&token_key, &token)?;
    if verdict == ConsentVerdict::Enabled {
        calendar_editing::set_choice(&conn, &email, Some(Choice::On))?;
        calendar_editing::mark_write_granted(&conn, &email)?;
        Ok(EditingStatus::On)
    } else {
        calendar_editing::set_choice(&conn, &email, None)?;
        Ok(EditingStatus::Off)
    }
}

/// Switch editing off (`paused`) or back on for an account that has it turned on, without asking
/// Google: the permission stays granted, PM just stops using it. Returns the account's status.
#[tauri::command]
pub fn set_calendar_editing_paused(
    app: AppHandle,
    window: tauri::Window,
    state: State<'_, AppState>,
    email: String,
    paused: bool,
) -> Result<EditingStatus> {
    require_main_window(&window)?;
    require_vault_owner(&app)?;
    let choice = if paused { Choice::Paused } else { Choice::On };
    {
        let conn = state.conn()?;
        if calendar_editing::choice(&conn, &email)?.is_none() {
            return Err(Error::Other(
                "Editing isn't turned on for this account.".into(),
            ));
        }
        calendar_editing::set_choice(&conn, &email, Some(choice))?;
    }
    let scope = google::token_scope(&google_calendar_token_key(&email))
        .ok()
        .flatten();
    Ok(calendar_editing::status(Some(choice), scope.as_deref()))
}

/// One-time, online: lift an existing single-account Google Calendar connection (the legacy fixed
/// keychain token + the old `google_calendar_ids` selection) into the new multi-account model. Learns
/// the account email from its primary calendar, re-keys the token to its per-account key, registers
/// the `gcal:<email>` source + calendars (preserving the old selection), and deletes the legacy key.
/// Idempotent + best-effort: a no-op once migrated, with no legacy token, or if the fetch fails (it
/// retries next time). Never holds the DB lock across the fetch (rule #4).
async fn migrate_legacy_google_calendar(app: &AppHandle) -> Result<()> {
    // Attempt the (network) fetch at most once per process: `calendar_overview` — a cheap read that
    // fires on every tab-mount/refresh — also calls this, and without the gate a transient fetch
    // failure would re-hit Google on every overview. The cheap keychain/DB checks below still run
    // each time; only the fetch is gated. `sync_calendar` also calls this, so a first-run failure
    // still retries on the next sync (and on the next app start).
    static FETCH_TRIED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

    if secrets::get_google_token_for(secrets::GOOGLE_TOKEN_CALENDAR)?.is_none() {
        return Ok(());
    }
    // A Google calendar account already registered? Drop the redundant legacy key and stop.
    {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        if !calendar::list_sources(&conn, Some("google"))?.is_empty() {
            secrets::clear_google_token_for(secrets::GOOGLE_TOKEN_CALENDAR).ok();
            return Ok(());
        }
    }
    if FETCH_TRIED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return Ok(());
    }
    let (raw, _) = calendar::fetch_calendar_list(secrets::GOOGLE_TOKEN_CALENDAR).await?;
    let Some(email) = raw.iter().find(|c| c.primary).map(|c| c.id.clone()) else {
        return Ok(()); // can't identify the account yet; try again next time
    };
    let account = calendar::google_account_id(&email);
    if let Some(blob) = secrets::get_google_token_for(secrets::GOOGLE_TOKEN_CALENDAR)? {
        secrets::set_google_token_for(&google_calendar_token_key(&email), blob.expose())?;
    }
    {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        let old_selection = calendar::selected_calendar_ids(&conn)?; // legacy remote ids
        calendar::upsert_source(&conn, &account, "google", Some(&email), &email)?;
        let inputs: Vec<_> = raw.iter().map(|c| c.to_input()).collect();
        // A fresh `gcal:<email>` source, so there is nothing to prune yet; upsert-only (false).
        calendar::register_calendars(&conn, &account, "google", &inputs, false, |it| {
            old_selection.iter().any(|id| id == &it.remote_id)
        })?;
    }
    secrets::clear_google_token_for(secrets::GOOGLE_TOKEN_CALENDAR).ok();
    Ok(())
}

// --- Outlook Calendar (Microsoft Graph OAuth, per-account) ---

/// Connect an Outlook / Microsoft 365 calendar account: consent (Graph `Calendars.Read`), learn the
/// account via `/me`, store the token, and register the account + its calendars (all selected).
#[tauri::command]
pub async fn connect_outlook_calendar(app: AppHandle) -> Result<calendar::CalendarAccount> {
    require_vault_owner(&app)?;
    let token = microsoft::run_consent(microsoft::CALENDAR_SCOPE, "Outlook Calendar").await?;
    let (email, name) = outlook_calendar::me_account(&token).await?;
    // Normalise the account identity so a differently-cased reconnect doesn't duplicate the account
    // (Graph's `mail`/`userPrincipalName` casing can vary); keep `name` for the human-readable label.
    let email = email.trim().to_lowercase();
    let token_key = outlook_calendar::account_token_key(&email);
    microsoft::save_token(&token_key, &token)?;
    let (raw, _) = outlook_calendar::list_calendars(&token_key).await?;
    let account = outlook_calendar::account_id(&email);
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    calendar::upsert_source(&conn, &account, "microsoft", Some(&email), &name)?;
    // Upsert-only on connect (never prune); the first `sync_calendar` reconcile prunes off a complete list.
    calendar::register_calendars(&conn, &account, "microsoft", &raw, false, |_| true)?;
    calendar::list_sources(&conn, Some("microsoft"))?
        .into_iter()
        .find(|a| a.id == account)
        .ok_or_else(|| Error::Other("the account registration could not be read back".into()))
}

/// Disconnect one Outlook calendar account.
#[tauri::command]
pub fn disconnect_outlook_calendar(
    app: AppHandle,
    state: State<'_, AppState>,
    email: String,
) -> Result<()> {
    require_vault_owner(&app)?;
    let conn = state.conn()?;
    // Clear the token first and propagate a real failure, then drop the source (see the Google
    // sibling): removing the DB row before an un-clearable token would orphan the token.
    secrets::clear_microsoft_token_for(&outlook_calendar::account_token_key(&email))?;
    calendar::remove_source(&conn, &outlook_calendar::account_id(&email))?;
    Ok(())
}

// --- iCal subscriptions — the no-OAuth path (works under Advanced Protection) ---

/// Subscribed feeds without their secret URLs, for Settings.
#[tauri::command]
pub fn list_ics_feeds() -> Result<Vec<IcsFeedInfo>> {
    calendar::feed_infos()
}

/// Add an iCal subscription and sync it immediately. `provider` tags it (`apple`/`outlook`/`other`,
/// defaulting to `other` when omitted). Persists nothing until the feed fetches cleanly, so a broken
/// URL leaves nothing behind.
#[tauri::command]
pub async fn add_ics_feed(
    app: AppHandle,
    label: String,
    url: String,
    provider: Option<String>,
) -> Result<()> {
    let provider = provider.unwrap_or_else(|| "other".to_string());
    let feed = calendar::build_feed(&label, &url, &provider)?;
    // Resolve the user's zone (for floating/all-day ICS times) and the mirror window under a short
    // lock, then drop it before the network sync (rule #4).
    let (tz, (time_min, time_max)) = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        (resolve_zone(&conn), calendar::time_window(&conn)?)
    };
    let (events, complete) = calendar::sync_feed(&feed, &time_min, &time_max, tz).await?;
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    calendar::save_new_feed(&feed)?;
    calendar::register_feed_source(&conn, &feed)?;
    calendar::replace_events(&conn, &feed.id, &events, complete)?;
    // "Last synced" means "last COMPLETE sync": a feed that parsed only partly is still added (the
    // events it gave are real), but stamping it clean would claim a picture we don't have.
    if complete {
        calendar::set_last_sync(&conn)?;
    } else {
        calendar::set_source_state(&conn, &feed.id, "error")?;
    }
    Ok(())
}

/// Remove a feed, its registry rows, and its mirrored events.
#[tauri::command]
pub fn remove_ics_feed(state: State<'_, AppState>, id: String) -> Result<()> {
    let conn = state.conn()?;
    calendar::remove_feed(&conn, &id)
}

/// Store the user's BYO Google "Desktop app" client credentials (keychain only).
#[tauri::command]
pub fn set_google_client(app: AppHandle, client_id: String, client_secret: String) -> Result<()> {
    require_vault_owner(&app)?;
    let id = client_id.trim();
    let secret = client_secret.trim();
    if id.is_empty() || secret.is_empty() {
        return Err(Error::Other(
            "Both the Client ID and Client secret are required.".into(),
        ));
    }
    secrets::set_google_client(id, secret)
}

/// The Google accounts PM already holds an own Cloud project for (Advanced Protection), so connecting
/// such an account to another Google service can reuse it instead of asking for the project again.
/// Emails only — never the client id or secret.
#[tauri::command]
pub fn google_saved_projects(state: State<'_, AppState>) -> Result<Vec<String>> {
    let emails = {
        let conn = state.conn()?;
        google_account_emails(&conn)?
    };
    // One entry per person, in the first spelling whose lookup resolves — that exact string is what
    // the connect passes back, so it must be one the lookup can find.
    let mut saved: Vec<String> = Vec::new();
    for email in emails {
        if saved.iter().any(|s| s.eq_ignore_ascii_case(&email)) {
            continue;
        }
        if secrets::get_google_client_for_account(&email)?.is_some() {
            saved.push(email);
        }
    }
    Ok(saved)
}

/// One stored Google token in the dev grant report.
#[cfg(debug_assertions)]
#[derive(Serialize)]
pub struct GrantReportRow {
    /// The keychain key: which service, and which account.
    pub token_key: String,
    /// Granted scopes, without Google's URL prefix (`calendar.readonly`, `drive.file`, …).
    pub scopes: Vec<String>,
    /// Which client minted it: `own`, `shared`, `gone` (no longer saved) or `unrecorded`.
    pub client: &'static str,
}

/// Dev builds only: what Google granted each stored Google token, for the live tests of how Google
/// merges an account's grants (#884's L-U: does a Drive or backup consent pick up the calendar write
/// scope?). Never a token, a client id or a secret. From the devtools console:
/// `await window.__TAURI_INTERNALS__.invoke("dev_google_grant_report")`.
#[cfg(debug_assertions)]
#[tauri::command]
pub fn dev_google_grant_report(state: State<'_, AppState>) -> Result<Vec<GrantReportRow>> {
    let keys = {
        let conn = state.conn()?;
        let mut keys = Vec::new();
        for email in google_account_emails(&conn)? {
            keys.extend(google_grant_token_keys(&conn, &email)?);
        }
        keys.sort();
        keys.dedup();
        keys
    };
    let mut rows = Vec::new();
    for token_key in keys {
        if let Some((scopes, client)) = google::grant_summary(&token_key)? {
            rows.push(GrantReportRow {
                token_key,
                scopes,
                client,
            });
        }
    }
    Ok(rows)
}

/// Forget the Google client credentials. The client is shared by every Google service, so this
/// invalidates them all: drop each Calendar account + every Drive account and the events/items they
/// mirror (ICS/Outlook events, which don't depend on this client, are kept).
///
/// Owner-only, like every connector removal: on a shared vault a joiner would drop the owner's rows
/// while the owner's tokens stayed live in the owner's keychain.
///
/// Each account's sign-in is cleared before its row, and a keychain failure stops the clear with
/// that account still listed. PM finds tokens only through these rows and the backup setting, so a
/// row dropped over a token that wouldn't clear stranded a live sign-in no disconnect, and not even
/// "Remove PM data", could reach (#893). Nothing is revoked at Google: this forgets the client on this
/// device, as before.
#[tauri::command]
pub fn clear_google_client(app: AppHandle, state: State<'_, AppState>) -> Result<()> {
    require_vault_owner(&app)?;
    let conn = state.conn()?;
    for acc in calendar::list_sources(&conn, Some("google"))? {
        if let Some(email) = &acc.email {
            secrets::clear_google_token_for(&google_calendar_token_key(email))?;
            // Also drop any per-account (Advanced-Protection) client secret, else it's orphaned in
            // the keychain with no UI path to remove it and a later reconnect reuses the stale creds.
            secrets::clear_google_client_for_account(email)?;
        }
        calendar::remove_source(&conn, &acc.id)?;
    }
    secrets::clear_google_token_for(google::CALENDAR_TOKEN_KEY).ok(); // any not-yet-migrated legacy token
    calendar_editing::clear_all(&conn)?;
    drive::forget_all_accounts(&conn)?;
    // A backup-only account has no Drive row, so the loop above never saw its sign-in. Clear it before
    // the destination below forgets which account it was.
    if let Some(backup) = crate::backup::schedule::gdrive_account(&conn)? {
        secrets::clear_google_token_for(&drive::account_token_key(&backup))?;
        secrets::clear_google_client_for_account(&backup)?;
    }
    // F-38: the Google-Drive BACKUP destination rides on this same client, so tearing the client down
    // must also disable it — otherwise the schedule keeps `gdrive_enabled` pointed at a now-tokenless
    // account and every scheduled backup fails on it (eprintln-only, invisible on a GUI build).
    crate::backup::schedule::clear_gdrive_destination(&conn)?;
    secrets::clear_google_client()?;
    // Drop events for the now-removed Google calendars; selected ICS/Outlook events are kept.
    let active: Vec<String> = calendar::selected_calendars(&conn)?
        .into_iter()
        .map(|c| c.id)
        .collect();
    calendar::prune_unselected(&conn, &active)
}

// --- shared sync over every provider ---

/// Pull events from a single selected calendar (provider-dispatched) and write them to the mirror.
/// Returns `(event count, complete)` — `complete` is the fetch's own verdict on whether it saw the
/// whole calendar, and gates the mirror's delete half plus the caller's state stamp. Never holds the
/// DB lock across the fetch (rule #4).
async fn sync_one_calendar(
    app: &AppHandle,
    cal: &calendar::Calendar,
    feed_by_id: &std::collections::HashMap<String, calendar::IcsFeed>,
    time_min: &str,
    time_max: &str,
    tz: chrono_tz::Tz,
) -> Result<(usize, bool)> {
    // Taken before the fetch: only a fetch that began after a save may settle it (rule R4).
    let fetch_started = std::time::Instant::now();
    let (events, complete) = match cal.provider.as_str() {
        "google" => {
            let email = calendar::account_email_of(&cal.source_id).ok_or_else(|| {
                Error::Other(format!("bad calendar source id: {}", cal.source_id))
            })?;
            let remote = cal.remote_id.as_deref().unwrap_or(&cal.id);
            calendar::fetch_events(
                &google_calendar_token_key(&email),
                &cal.id,
                remote,
                time_min,
                time_max,
            )
            .await?
        }
        "microsoft" => {
            let email = calendar::account_email_of(&cal.source_id).ok_or_else(|| {
                Error::Other(format!("bad calendar source id: {}", cal.source_id))
            })?;
            let remote = cal.remote_id.as_deref().unwrap_or(&cal.id);
            outlook_calendar::fetch_events(
                &outlook_calendar::account_token_key(&email),
                &cal.id,
                remote,
                time_min,
                time_max,
            )
            .await?
        }
        // Any other provider is an iCal subscription (its source id is the feed id).
        _ => {
            let feed = feed_by_id.get(&cal.source_id).ok_or_else(|| {
                Error::Other(format!(
                    "calendar subscription {} has no stored URL",
                    cal.source_id
                ))
            })?;
            calendar::sync_feed(feed, time_min, time_max, tz).await?
        }
    };
    let n = events.len();
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    write_fetched_events(
        &conn,
        &state.calendar_edit,
        cal,
        events,
        complete,
        fetch_started,
    )?;
    Ok((n, complete))
}

/// Write one calendar's fetch into the mirror, through the saves of the last ten minutes: a fetch
/// that began before a save can't put the old version back (rule R4). The caller holds the DB lock,
/// and the recent writes are locked inside it (lock order: the DB, then the recent writes).
fn write_fetched_events(
    conn: &rusqlite::Connection,
    edit: &super::CalendarEditState,
    cal: &calendar::Calendar,
    events: Vec<CalendarEvent>,
    complete: bool,
    fetch_started: std::time::Instant,
) -> Result<()> {
    let mut recent = edit
        .recent
        .lock()
        .map_err(|_| Error::Other("recent writes lock poisoned".into()))?;
    if recent.is_empty() {
        drop(recent);
        return calendar::replace_events(conn, &cal.id, &events, complete);
    }
    let merged = recent.merge(&cal.id, events, complete, fetch_started, Stamp::now());
    drop(recent);
    calendar::replace_events(conn, &cal.id, &merged.rows, complete)?;
    if !merged.deletes.is_empty() {
        calendar::apply_write_effect(conn, &cal.id, &[], &merged.deletes)?;
    }
    Ok(())
}

/// Re-fetch each connected OAuth account's calendar LIST and reconcile the registry before events are
/// pulled: a calendar created upstream appears (selected, so it shows on the Calendar tab), and a
/// calendar deleted upstream is pruned — but ONLY when the list came back provably COMPLETE, so a
/// truncated page-run or an unreachable account can never delete a real calendar (its selected/quiet
/// choices and mirrored events). Best-effort per account: a failed list fetch is skipped here, and the
/// account's state is still settled by the event-sync pass. Never holds the DB lock across a fetch
/// (rule #4). ICS feeds carry no separate list to reconcile (one feed is one calendar).
async fn reconcile_calendar_lists(app: &AppHandle) {
    let accounts: Vec<calendar::CalendarAccount> = {
        let state = app.state::<AppState>();
        let Ok(conn) = state.conn() else {
            return;
        };
        let mut v = calendar::list_sources(&conn, Some("google")).unwrap_or_default();
        v.extend(calendar::list_sources(&conn, Some("microsoft")).unwrap_or_default());
        v
    };
    for acc in accounts {
        let Some(email) = acc.email.clone() else {
            continue;
        };
        let fetched: Result<(Vec<calendar::RawCalendarInput>, bool)> = match acc.provider.as_str() {
            "google" => calendar::fetch_calendar_list(&google_calendar_token_key(&email))
                .await
                .map(|(raw, complete)| (raw.iter().map(|c| c.to_input()).collect(), complete)),
            "microsoft" => {
                outlook_calendar::list_calendars(&outlook_calendar::account_token_key(&email)).await
            }
            _ => continue,
        };
        // An unreachable account (token/refresh/list failure) is skipped, NOT pruned — the event pass
        // marks it 'unreachable'. Only a successful AND complete list may delete a vanished calendar.
        let Ok((items, complete)) = fetched else {
            continue;
        };
        let state = app.state::<AppState>();
        let Ok(conn) = state.conn() else {
            continue;
        };
        let _ = calendar::register_calendars(
            &conn,
            &acc.id,
            &acc.provider,
            &items,
            complete,
            // A newly-discovered calendar is shown by default (selected); the user can untick it.
            |_| true,
        );
    }
}

/// Pull events from every selected calendar (all providers + ICS subscriptions) into the mirror.
/// Returns the total events synced. Best-effort per source and never holds the DB lock across a fetch
/// (rule #4); a source whose every calendar failed flips to `unreachable` while the rest keep their
/// last-good events. Surfaces an error only if at least one source failed (the successes are committed).
/// A source that fetched but couldn't see the whole calendar is stamped `error` ("the pass ran but
/// didn't finish") rather than returned as an error — the write genuinely succeeded, so the honest
/// signal is the state, not a toast.
#[tauri::command]
pub async fn sync_calendar(app: AppHandle) -> Result<usize> {
    // One sync at a time (the Refresh button, the poll): an older fetch finishing after a newer one
    // has settled a save would show the event as it was before the save (plan A23).
    let state = app.state::<AppState>();
    let _single = state.calendar_edit.sync_lock.lock().await;
    let _ = migrate_legacy_google_calendar(&app).await;
    // Pick up calendars created or deleted upstream before syncing events, so a new calendar shows up
    // and a deleted one stops pinning the account 'unreachable' every sync (deletions honoured only on
    // a provably complete list — see `reconcile_calendar_lists`).
    reconcile_calendar_lists(&app).await;

    // Phase 1 (brief lock): snapshot what to sync.
    let (calendars, feeds, (time_min, time_max), tz) = {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        (
            calendar::selected_calendars(&conn)?,
            calendar::load_feeds()?,
            calendar::time_window(&conn)?,
            resolve_zone(&conn),
        )
    };

    // The set of calendar ids we intend to keep events for — anything else is pruned.
    let active: Vec<String> = calendars.iter().map(|c| c.id.clone()).collect();
    if active.is_empty() {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        calendar::clear_all_events(&conn)?;
        calendar::set_last_sync(&conn)?;
        return Ok(0);
    }

    let feed_by_id: std::collections::HashMap<String, calendar::IcsFeed> =
        feeds.into_iter().map(|f| (f.id.clone(), f)).collect();

    let mut total = 0usize;
    let mut ok_sources: std::collections::HashSet<String> = std::collections::HashSet::new();
    // A source whose fetch SUCCEEDED but couldn't see the whole calendar. Its events were written
    // (merged, never reaped — see `calendar::replace_events`), so it is neither a failure nor a
    // clean sync; it gets its own bucket and the 'error' state, meaning "the pass ran but didn't
    // finish", exactly as Drive and the local folder already use it.
    let mut partial_sources: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut failed_sources: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut last_err: Option<Error> = None;

    // Fetch a few calendars at a time (the fetch half holds no DB lock; each `replace_events`
    // write inside stays its own short lock). `buffered` keeps results in calendar order, so the
    // per-calendar accounting below matches the old sequential loop.
    use futures_util::stream::StreamExt;
    const CALENDAR_FETCH_CONCURRENCY: usize = 3;
    // The futures are collected eagerly (they're inert until polled) so the stream holds plain
    // future values — leaving the mapping closure inside the stream type trips a higher-ranked
    // `FnOnce` inference error in the generated command wrapper. The re-borrows keep each
    // `async move` block owning only references (`move` alone would swallow `app` whole).
    let fetches: Vec<_> = calendars
        .iter()
        .map(|cal| {
            let (app, feed_by_id) = (&app, &feed_by_id);
            let (time_min, time_max) = (&time_min, &time_max);
            async move {
                let r = sync_one_calendar(app, cal, feed_by_id, time_min, time_max, tz).await;
                (cal, r)
            }
        })
        .collect();
    let mut results = futures_util::stream::iter(fetches).buffered(CALENDAR_FETCH_CONCURRENCY);
    while let Some((cal, result)) = results.next().await {
        match result {
            Ok((n, complete)) => {
                total += n;
                if complete {
                    ok_sources.insert(cal.source_id.clone());
                } else {
                    partial_sources.insert(cal.source_id.clone());
                }
            }
            Err(e) => {
                failed_sources.insert(cal.source_id.clone());
                last_err = Some(e);
            }
        }
    }

    {
        let state = app.state::<AppState>();
        let conn = state.conn()?;
        // Reconcile deselected/removed calendars against the CURRENT selection, not the phase-1
        // snapshot — a calendar the user un-ticked/disconnected during the unlocked fetch is then
        // pruned this round instead of lingering until the next sync.
        let active_now: Vec<String> = calendar::selected_calendars(&conn)?
            .into_iter()
            .map(|c| c.id)
            .collect();
        calendar::prune_unselected(&conn, &active_now)?;
        // A source with ANY failed calendar this round is 'unreachable' — check failures FIRST, so
        // a partially-failed account (some calendars ok, some not) isn't stamped a clean 'ok' and
        // hidden from the Connectors warning. A source that failed keeps its last-good events.
        // Incompleteness is checked next, for the same reason one rung down: a source with one
        // truncated calendar must not be stamped 'ok' just because its other calendars finished.
        for acc in calendar::list_sources(&conn, None)? {
            if failed_sources.contains(&acc.id) {
                calendar::set_source_state(&conn, &acc.id, "unreachable")?;
            } else if partial_sources.contains(&acc.id) {
                calendar::set_source_state(&conn, &acc.id, "error")?;
            } else if ok_sources.contains(&acc.id) {
                calendar::set_source_synced(&conn, &acc.id)?;
            }
        }
        // Only stamp a clean global sync when every selected source refreshed IN FULL — "last
        // synced" has to keep meaning "last complete sync", or a permanently-truncated calendar
        // would show a fresh timestamp over a mirror that is quietly missing its tail.
        if last_err.is_none() && partial_sources.is_empty() {
            calendar::set_last_sync(&conn)?;
        }
        // The mirror just moved, so what the briefing says about today may have moved with it (a
        // new meeting, a cancelled one, a time change). Flag it rather than regenerating here: the
        // scheduler coalesces, and re-briefs only if the facts genuinely differ — so the ordinary
        // case, a poll that pulled nothing new, costs nothing.
        briefing::nudge(&state);
    }

    if let Some(e) = last_err {
        return Err(e);
    }
    Ok(total)
}

/// Every mirrored event across the widened window — the read backing the unified calendar view
/// (card 8). The focus view keeps the narrow forward agenda ([`list_calendar_events`]); this returns
/// the whole band (previous month included) and the client filters to the visible range.
///
/// Each row says whether PM could edit it (`edit_block`), as far as the mirror can tell; the editor
/// asks Google again when it opens.
#[tauri::command]
pub fn list_all_calendar_events(state: State<'_, AppState>) -> Result<Vec<ListedEvent>> {
    let conn = state.conn()?;
    let rows = calendar::list_all_events(&conn)?;
    let blocks = super::calendar_edit::edit_blocks(&conn, &rows)?;
    Ok(rows
        .into_iter()
        .zip(blocks)
        .map(|(event, edit_block)| ListedEvent { event, edit_block })
        .collect())
}

/// A row of the unified calendar view.
#[derive(Serialize)]
pub struct ListedEvent {
    #[serde(flatten)]
    pub event: CalendarEvent,
    /// Why nothing about it may change, or `None` when something can, assuming editing is on for
    /// its account (the view merges each account's real status from `calendar_overview`).
    pub edit_block: Option<ReadOnlyReason>,
}

/// The active PM flags anchored on a calendar event's iCal UID — shown in the event detail popup so a
/// linked "prepare ahead" / "happening today" flag is visible where the event is. Empty when the event
/// has no UID or no flags. (A calendar flag's `anchor` IS the event's iCal UID — flags.rs.)
#[tauri::command]
pub fn event_flags(state: State<'_, AppState>, uid: String) -> Result<Vec<flags::Flag>> {
    if uid.trim().is_empty() {
        return Ok(Vec::new());
    }
    let conn = state.conn()?;
    Ok(flags::list_active(&conn, Some(flags::ANCHOR_CALENDAR))?
        .into_iter()
        .filter(|f| f.anchor == uid)
        .collect())
}

/// The upcoming events in the mirror, for the focus-view agenda. Each row carries `ended` — the agenda
/// widens the strict "not yet ended" gate to keep events that finished earlier today (in the user's
/// zone) so the view can show them de-emphasised until the user's local midnight.
#[tauri::command]
pub fn list_calendar_events(state: State<'_, AppState>) -> Result<Vec<calendar::AgendaEvent>> {
    let conn = state.conn()?;
    let zone = resolve_zone(&conn);
    calendar::focus_agenda(&conn, calendar::AGENDA_DAYS, zone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar::CalendarFacts;
    use std::time::Instant;

    const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
    const CAL: &str = "gcal:me@x.com:me@x.com";

    fn store() -> (tempfile::TempDir, rusqlite::Connection, calendar::Calendar) {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        calendar::upsert_source(
            &conn,
            "gcal:me@x.com",
            "google",
            Some("me@x.com"),
            "me@x.com",
        )
        .unwrap();
        let cal = calendar::Calendar {
            id: CAL.into(),
            source_id: "gcal:me@x.com".into(),
            provider: "google".into(),
            remote_id: Some("me@x.com".into()),
            name: "Me".into(),
            color: None,
            selected: true,
            is_primary: true,
            quiet: false,
            kind: None,
            facts: CalendarFacts::default(),
        };
        calendar::upsert_calendar(&conn, &cal).unwrap();
        (dir, conn, cal)
    }

    fn event(id: &str, summary: &str, etag: &str, updated: &str) -> CalendarEvent {
        CalendarEvent {
            id: format!("{CAL}:{id}"),
            calendar_id: CAL.into(),
            summary: summary.into(),
            start: "2026-10-12T09:00:00Z".into(),
            etag: Some(etag.into()),
            updated: Some(updated.into()),
            ..Default::default()
        }
    }

    fn titles(conn: &rusqlite::Connection) -> Vec<String> {
        calendar::list_all_events(conn)
            .unwrap()
            .into_iter()
            .map(|e| e.summary)
            .collect()
    }

    #[test]
    fn a_sync_that_began_before_a_save_cant_put_the_old_version_back() {
        let (_d, conn, cal) = store();
        let edit = super::super::CalendarEditState::default();
        let before = Instant::now();
        let old = event("a", "Dentist", "\"1\"", "2026-10-09T10:00:00Z");
        write_fetched_events(&conn, &edit, &cal, vec![old.clone()], true, before).unwrap();
        // The save lands after that sync began.
        let saved = event("a", "Dentist (moved)", "\"2\"", "2026-10-09T11:00:00Z");
        edit.recent
            .lock()
            .unwrap()
            .record_upsert(saved.clone(), Stamp::now());
        // The older fetch finishes now, still showing the old version: the save stays.
        write_fetched_events(&conn, &edit, &cal, vec![old], true, before).unwrap();
        assert_eq!(titles(&conn), vec!["Dentist (moved)"]);
        assert!(
            !edit.recent.lock().unwrap().is_empty(),
            "not settled by an older fetch"
        );
        // A fetch begun after the save shows it: settled.
        write_fetched_events(&conn, &edit, &cal, vec![saved], true, Instant::now()).unwrap();
        assert_eq!(titles(&conn), vec!["Dentist (moved)"]);
        assert!(edit.recent.lock().unwrap().is_empty());
    }

    #[test]
    fn a_deleted_event_stays_gone_even_from_an_incomplete_fetch() {
        let (_d, conn, cal) = store();
        let edit = super::super::CalendarEditState::default();
        let before = Instant::now();
        let a = event("a", "Dentist", "\"1\"", "2026-10-09T10:00:00Z");
        let b = event("b", "Gym", "\"1\"", "2026-10-09T10:00:00Z");
        write_fetched_events(&conn, &edit, &cal, vec![a.clone(), b.clone()], true, before).unwrap();
        edit.recent.lock().unwrap().record_delete(
            CAL,
            &a.id,
            a.etag.clone(),
            a.updated.clone(),
            Stamp::now(),
        );
        // An incomplete fetch only upserts, so the deleted row is removed explicitly.
        write_fetched_events(&conn, &edit, &cal, vec![a], false, before).unwrap();
        assert_eq!(titles(&conn), vec!["Gym"]);
    }
}
