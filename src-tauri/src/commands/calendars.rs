// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The read-only calendar mirror: Google, Outlook and iCal accounts, their calendars, and
//! the shared sync pass over every provider.
//!
//! `set_google_client` / `clear_google_client` live here because that is where they sit in
//! the Google Calendar flow and `clear_google_client` reads calendar rows — but the BYO
//! OAuth client they manage is ONE client serving Calendar *and* Drive, so a change here
//! reaches `connectors` and `backup::schedule` too.

use serde::Serialize;
use tauri::{AppHandle, Manager, State};

use crate::calendar::{self, CalendarEvent, IcsFeedInfo};
use crate::error::{Error, Result};
use crate::google;
use crate::{briefing, drive, flags, microsoft, outlook_calendar, secrets, AppState};

use super::shared::own_client;
use super::shared::resolve_zone;
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
}

/// The unified calendar state across every provider. Runs the one-time legacy Google migration first
/// so an upgrading single-account user appears in the new model.
#[tauri::command]
pub async fn calendar_overview(app: AppHandle) -> Result<CalendarOverview> {
    let _ = migrate_legacy_google_calendar(&app).await;
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    let (mirror_start, mirror_end) = calendar::time_window(&conn)?;
    Ok(CalendarOverview {
        google_client_configured: google::has_client()?,
        microsoft_client_configured: microsoft::has_client()?,
        accounts: calendar::list_sources(&conn, None)?,
        calendars: calendar::list_calendars(&conn)?,
        last_sync: calendar::last_sync(&conn)?,
        window_days: calendar::AGENDA_DAYS,
        mirror_start,
        mirror_end,
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

/// The core connect flow, shared by the new per-account command and the back-compat `connect_google`:
/// run consent, learn the account from its primary calendar (id == email), store the token under that
/// account's key, and register the account + its calendars (all selected by default).
async fn do_connect_google_calendar(
    app: &AppHandle,
    own: Option<(String, String)>,
) -> Result<calendar::CalendarAccount> {
    let token = match &own {
        Some((id, secret)) => {
            google::run_consent_with_client(
                google::CALENDAR_SCOPE,
                "Google Calendar",
                id.clone(),
                secret.clone(),
            )
            .await?
        }
        None => google::run_consent(google::CALENDAR_SCOPE, "Google Calendar").await?,
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
    let account = calendar::google_account_id(&email);
    if let Some((id, secret)) = &own {
        secrets::set_google_client_for_account(&email, id, secret)?;
    }
    google::save_token(&google_calendar_token_key(&email), &token)?;
    let state = app.state::<AppState>();
    let conn = state.conn()?;
    calendar::upsert_source(&conn, &account, "google", Some(&email), &email)?;
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
/// Cloud project (`client_id`/`client_secret`) — the Advanced-Protection path, mirroring `connect_drive`.
#[tauri::command]
pub async fn connect_google_calendar_account(
    app: AppHandle,
    client_id: Option<String>,
    client_secret: Option<String>,
) -> Result<calendar::CalendarAccount> {
    require_vault_owner(&app)?;
    do_connect_google_calendar(&app, own_client(client_id, client_secret)?).await
}

/// Disconnect one Google Calendar account: drop its registry source (cascading its calendars +
/// mirrored events) and forget its token plus any per-account (Advanced-Protection) client.
#[tauri::command]
pub async fn disconnect_google_calendar_account(
    state: State<'_, AppState>,
    email: String,
) -> Result<()> {
    // L-3: sever the grant at Google's end BEFORE forgetting the local token (best-effort, like wipe).
    if let Ok(Some(blob)) = secrets::get_google_token_for(&google_calendar_token_key(&email)) {
        let _ = google::revoke(blob.expose()).await;
    }
    let conn = state.conn()?;
    // Clear the OAuth token FIRST and propagate a real failure (a locked keychain): dropping the DB
    // source before an un-clearable token would orphan the token with no source left to re-clear it.
    // `secrets::delete` treats a missing entry as success, so a returned Err is a genuine failure.
    secrets::clear_google_token_for(&google_calendar_token_key(&email))?;
    calendar::remove_source(&conn, &calendar::google_account_id(&email))?;
    secrets::clear_google_client_for_account(&email).ok(); // per-AP client; absent for shared-client accounts
    Ok(())
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
pub fn disconnect_outlook_calendar(state: State<'_, AppState>, email: String) -> Result<()> {
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

/// Forget the Google client credentials. The client is shared by every Google service, so this
/// invalidates them all: drop each Calendar account + every Drive account and the events/items they
/// mirror (ICS/Outlook events, which don't depend on this client, are kept).
#[tauri::command]
pub fn clear_google_client(state: State<'_, AppState>) -> Result<()> {
    let conn = state.conn()?;
    for acc in calendar::list_sources(&conn, Some("google"))? {
        calendar::remove_source(&conn, &acc.id)?;
        if let Some(email) = acc.email {
            secrets::clear_google_token_for(&google_calendar_token_key(&email)).ok();
            // Also drop any per-account (Advanced-Protection) client secret, else it's orphaned in
            // the keychain with no UI path to remove it and a later reconnect reuses the stale creds.
            secrets::clear_google_client_for_account(&email).ok();
        }
    }
    secrets::clear_google_token_for(google::CALENDAR_TOKEN_KEY).ok(); // any not-yet-migrated legacy token
    drive::forget_all_accounts(&conn).ok();
    // F-38: the Google-Drive BACKUP destination rides on this same client, so tearing the client down
    // must also disable it — otherwise the schedule keeps `gdrive_enabled` pointed at a now-tokenless
    // account and every scheduled backup fails on it (eprintln-only, invisible on a GUI build).
    crate::backup::schedule::clear_gdrive_destination(&conn).ok();
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
    calendar::replace_events(&conn, &cal.id, &events, complete)?;
    Ok((n, complete))
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
#[tauri::command]
pub fn list_all_calendar_events(state: State<'_, AppState>) -> Result<Vec<CalendarEvent>> {
    let conn = state.conn()?;
    calendar::list_all_events(&conn)
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
