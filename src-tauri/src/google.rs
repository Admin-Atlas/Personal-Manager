// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Google OAuth — PM's first connector (spec §8.6). BYO credentials: the user
//! supplies a Google Cloud "Desktop app" OAuth client (id + secret), so no Google
//! secret ships in the repo (rule #1). The flow is the recommended desktop pattern:
//! a **loopback redirect with PKCE** — PM opens the system browser to Google's
//! consent screen with `redirect_uri=http://127.0.0.1:<ephemeral-port>`, runs a
//! one-shot local HTTP server to catch the redirect, and exchanges the code for
//! tokens. Scopes are read-only except two opt-ins: `drive.file` for encrypted backups and
//! `calendar.events` for calendar editing, which the user turns on per account. Access tokens are
//! refreshed transparently; the token blob lives only in the keychain, never on disk.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::oauth_loopback;
use crate::secret::Secret;
use crate::secrets;

const AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
/// Read-only calendar scope: every calendar connect asks for it, and the mirror syncs through it.
pub const CALENDAR_SCOPE: &str = "https://www.googleapis.com/auth/calendar.readonly";
/// Calendar write scope: create, change and delete events on the calendars the account can edit.
/// Asked for only by the per-account "Turn on editing" consent, always together with
/// [`CALENDAR_SCOPE`] (see [`calendar_editing_scopes`]); it can't list calendars by itself. Holding it
/// is not enough to edit: `crate::calendar_editing` also requires the user's own switch.
pub const CALENDAR_EVENTS_SCOPE: &str = "https://www.googleapis.com/auth/calendar.events";
/// Read-only Drive scope — PM reads file metadata + content, never writes. The full-read
/// `drive.readonly` (not `drive.metadata.readonly`) because index-only ingestion needs each
/// file's body to embed it.
pub const DRIVE_SCOPE: &str = "https://www.googleapis.com/auth/drive.readonly";
/// Read-only Sheets scope — requested ALONGSIDE `drive.readonly` when connecting a Drive account, so
/// PM can read a Google Sheet's tab names + header row via the Sheets API for the metadata-only Sheets
/// index (never the full grid). Because a refresh token cannot broaden its grant, adding this scope
/// means every EXISTING Drive account must re-consent to gain it; PM detects who needs it — offline,
/// no network — with [`token_has_scope`] and surfaces a per-account "Reconnect for Sheets" prompt.
/// A Drive consent's `include_granted_scopes=true` ([`merges_granted_scopes`]) unions it onto the
/// account's existing Drive grant.
pub const SHEETS_SCOPE: &str = "https://www.googleapis.com/auth/spreadsheets.readonly";
/// The Drive **write** scope, least-privilege, granted just for encrypted backup (PM's one other
/// Google write scope is [`CALENDAR_EVENTS_SCOPE`]). `drive.file` can create and manage only
/// files/folders the app itself created (PM's "Personal Manager Backups" folder and its `.pmbackup`
/// archives); it can never touch the user's other Drive content. Requested via a dedicated
/// re-consent (the connector scopes are read-only), which UNIONS it with any existing
/// `drive.readonly` grant on the account because a Drive consent sets `include_granted_scopes=true`
/// ([`merges_granted_scopes`]).
pub const DRIVE_FILE_SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
/// The keychain key for the Calendar service's token — passed into the per-service token
/// helpers below (the connector-generic flow takes a key so Drive accounts get their own).
pub const CALENDAR_TOKEN_KEY: &str = secrets::GOOGLE_TOKEN_CALENDAR;

/// The scope set of the "Turn on editing" consent: read and write, stated in full. Calendar consents
/// never ask Google to fold in earlier grants ([`merges_granted_scopes`]), so the read scope the mirror
/// syncs through has to be asked for again by name.
pub fn calendar_editing_scopes() -> String {
    format!("{CALENDAR_SCOPE} {CALENDAR_EVENTS_SCOPE}")
}

/// Whether the space-separated scope set `scopes` (a token's granted `scope`, or a consent request)
/// contains `wanted`, compared whole so `calendar` never matches `calendar.readonly`.
pub fn scope_set_has(scopes: &str, wanted: &str) -> bool {
    scopes.split_ascii_whitespace().any(|s| s == wanted)
}

/// Whether a consent for `scope` asks Google to fold the account's earlier grants into the new token
/// (`include_granted_scopes=true`). Drive and backup consents do: a backup's `drive.file` must land on
/// the same token as the Drive connector's read scopes, because both use one keychain key. Calendar
/// consents don't. Google calls incremental authorisation unsupported for installed apps, so what a
/// merge carries is undefined, and a calendar token is what PM would edit with: a read-only reconnect
/// must not come back holding a write scope nobody asked for this time. They state their whole scope
/// set instead ([`calendar_editing_scopes`]).
fn merges_granted_scopes(scope: &str) -> bool {
    !scope_set_has(scope, CALENDAR_SCOPE) && !scope_set_has(scope, CALENDAR_EVENTS_SCOPE)
}

/// The stored OAuth token blob (one keychain entry, JSON). `expiry` is Unix seconds.
/// The bearer/refresh values are [`Secret`], so the derived `Debug` here can never
/// print them — and serde stays transparent, so the JSON blob round-trips unchanged.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Token {
    pub access_token: Secret,
    #[serde(default)]
    pub refresh_token: Option<Secret>,
    pub expiry: i64,
    #[serde(default)]
    pub scope: Option<String>,
    /// The OAuth client that minted this grant. A refresh token only works with the client that
    /// issued it, and one account can hold grants from both the shared client and its own
    /// (Advanced-Protection) client, so a refresh picks the client by this id rather than by which
    /// clients happen to be saved. Absent on tokens saved before 3.139.9; those resolve as before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
}

/// True once the user has pasted a client id + secret.
pub fn has_client() -> Result<bool> {
    Ok(
        secrets::get_google_client_id()?.is_some()
            && secrets::get_google_client_secret()?.is_some(),
    )
}

fn client_creds() -> Result<(String, Secret)> {
    let id = secrets::get_google_client_id()?.ok_or_else(|| {
        Error::Other("Add your Google client ID and secret in Settings first.".into())
    })?;
    let secret = secrets::get_google_client_secret()?.ok_or_else(|| {
        Error::Other("Add your Google client ID and secret in Settings first.".into())
    })?;
    Ok((id, secret))
}

/// Which saved client refreshes a token: the account's own (Advanced-Protection) client, or the
/// shared one.
#[derive(Debug, PartialEq, Eq)]
enum ClientChoice {
    Own,
    Shared,
    /// The token names a client that is no longer saved — refreshing with any other would fail.
    Gone,
}

/// Pick the client for a token from the id that minted it (`minted_by`) and the ids of the clients
/// saved now. A token saved before minting ids were recorded (`None`) keeps the old rule: the
/// account's own client when it has one, else the shared one. Pure, so every case is table-tested.
fn choose_client(
    minted_by: Option<&str>,
    own_id: Option<&str>,
    shared_id: Option<&str>,
) -> ClientChoice {
    match minted_by {
        Some(id) if own_id == Some(id) => ClientChoice::Own,
        Some(id) if shared_id == Some(id) => ClientChoice::Shared,
        Some(_) => ClientChoice::Gone,
        None if own_id.is_some() => ClientChoice::Own,
        None => ClientChoice::Shared,
    }
}

/// The OAuth client that refreshes `token` (stored under `token_key`). The account email is the
/// suffix after `::` in the key (`google_oauth_token_drive::<email>` etc.); the legacy fixed calendar
/// key has no suffix, so it has no own client.
fn client_creds_for_token(token_key: &str, token: &Token) -> Result<(String, Secret)> {
    let own = match token_key.rsplit_once("::") {
        Some((_, email)) => secrets::get_google_client_for_account(email)?,
        None => None,
    };
    let shared_id = secrets::get_google_client_id()?;
    match choose_client(
        token.client_id.as_deref(),
        own.as_ref().map(|(id, _)| id.as_str()),
        shared_id.as_deref(),
    ) {
        ClientChoice::Own => Ok(own.expect("choose_client only picks Own when one is saved")),
        ClientChoice::Shared => client_creds(),
        ClientChoice::Gone => Err(Error::Other(
            "The Google Cloud project this account signed in with is no longer saved in PM — \
             reconnect the account in Settings."
                .into(),
        )),
    }
}

/// The client for a sign-in whose token will be saved under `token_key` (an account's existing key):
/// the client that minted the token already there, so the new consent widens that same grant
/// (`include_granted_scopes` only unions scopes within one project) instead of replacing it with a
/// grant to another project — which would drop the old grant's scopes from the key and leave that
/// grant live at Google with no token left to revoke it. With no usable token there, the account's
/// own client when one is saved (an Advanced-Protection account can't use the shared project at
/// all), else the shared one.
fn client_creds_for_saving(token_key: &str, email: &str) -> Result<(String, Secret)> {
    if let Some(raw) = secrets::get_google_token_for(token_key)? {
        if let Ok(existing) = serde_json::from_str::<Token>(raw.expose()) {
            if let Ok(creds) = client_creds_for_token(token_key, &existing) {
                return Ok(creds);
            }
        }
    }
    match secrets::get_google_client_for_account(email)? {
        Some(own) => Ok(own),
        None => client_creds(),
    }
}

fn http() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(Error::from)
}

/// Google's OAuth token-revocation endpoint (RFC 7009). Revoking a token here severs the grant at
/// Google's end, not just locally.
const REVOKE_ENDPOINT: &str = "https://oauth2.googleapis.com/revoke";

/// Best-effort revoke of a stored Google token blob at Google's end, so "Remove PM data" actually
/// severs the grant instead of only forgetting the local copy. Revoking the **refresh** token
/// invalidates the entire grant (every access token minted from it), so PM disappears from the
/// account's "Connected apps"; we fall back to the access token when no refresh token was stored.
/// `token_json` is the raw keychain blob (a [`Token`] as JSON). The caller runs this before deleting
/// the keychain entry and treats any error as non-fatal — the local secret is removed regardless, so
/// a revoke that can't reach the network still leaves nothing on this device.
pub async fn revoke(token_json: &str) -> Result<()> {
    let token: Token = serde_json::from_str(token_json)
        .map_err(|e| Error::Other(format!("token blob is not valid JSON: {e}")))?;
    let to_revoke = token
        .refresh_token
        .as_ref()
        .map(|s| s.expose().to_string())
        .unwrap_or_else(|| token.access_token.expose().to_string());
    let resp = http()?
        .post(REVOKE_ENDPOINT)
        .form(&[("token", to_revoke.as_str())])
        .send()
        .await
        .map_err(Error::from)?;
    // 200 = revoked; 400 = the token was already invalid/expired. Both mean the grant is not live,
    // which is exactly the desired end state, so neither is an error worth surfacing.
    if resp.status().is_success() || resp.status() == reqwest::StatusCode::BAD_REQUEST {
        Ok(())
    } else {
        Err(Error::Other(format!(
            "Google token revocation returned HTTP {}",
            resp.status()
        )))
    }
}

/// Run the OAuth consent flow and RETURN the token without persisting it: open the browser to
/// Google's consent screen for `scope`, catch the loopback redirect, and exchange the code. The
/// caller chooses which keychain key to store it under — for a Drive account that key is derived
/// from the account the token grants (known only after a follow-up `about` call), so persisting is
/// the caller's job. `success_label` names the connected product on the browser success page.
/// Errors (no client configured, browser failed, cancelled, timeout) surface to the UI.
pub async fn run_consent(scope: &str, success_label: &str) -> Result<Token> {
    run_consent_inner(scope, success_label, client_creds()?, None).await
}

/// As [`run_consent`], for a token PM will save under an account's existing key (`token_key`): signs
/// in through the client that minted the token already there, else the account's own client, else
/// the shared one ([`client_creds_for_saving`]), and starts Google's chooser on that account. The
/// caller still checks which account actually consented — the chooser lets the user pick another.
pub async fn run_consent_for_key(
    token_key: &str,
    email: &str,
    scope: &str,
    success_label: &str,
) -> Result<Token> {
    run_consent_inner(
        scope,
        success_label,
        client_creds_for_saving(token_key, email)?,
        Some(email),
    )
    .await
}

/// As [`run_consent`], through the own Cloud project PM already holds for `email` — the one-click
/// "Use <email>'s project" path, so an Advanced-Protection account never needs its project pasted a
/// second time. Refuses when no project is saved for the account (it was forgotten since the list was
/// read): the button promised that project, and the shared one would be blocked or wrong.
pub async fn run_consent_with_saved_project(
    email: &str,
    scope: &str,
    success_label: &str,
) -> Result<Token> {
    let own = secrets::get_google_client_for_account(email)?.ok_or_else(|| {
        Error::Other(format!(
            "PM no longer holds a Cloud project for {email}. Paste its Client ID and secret instead."
        ))
    })?;
    run_consent_inner(scope, success_label, own, Some(email)).await
}

/// As [`run_consent`], but using an account's OWN client (id + secret) supplied explicitly — the path
/// for connecting an Advanced-Protection account whose Cloud project isn't the shared one. The caller
/// persists the per-account client (keyed by the email it learns) so later refreshes reuse it.
pub async fn run_consent_with_client(
    scope: &str,
    success_label: &str,
    client_id: String,
    client_secret: String,
) -> Result<Token> {
    run_consent_inner(
        scope,
        success_label,
        (client_id, Secret::from(client_secret)),
        None,
    )
    .await
}

async fn run_consent_inner(
    scope: &str,
    success_label: &str,
    (client_id, client_secret): (String, Secret),
    login_hint: Option<&str>,
) -> Result<Token> {
    let (verifier, challenge) = oauth_loopback::pkce()?;
    let state = oauth_loopback::random_token(16)?;

    // Bind the loopback listener first so the port is known before we build the URL.
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| Error::Other(format!("Could not start the local sign-in server: {e}")))?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}");

    let auth_url = build_auth_url(
        &client_id,
        &redirect_uri,
        &challenge,
        &state,
        scope,
        login_hint,
    )?;
    open::that(&auth_url)
        .map_err(|e| Error::Other(format!("Couldn't open your browser to sign in: {e}")))?;

    // Wait for Google to redirect back with the code (blocking accept, off-runtime).
    let expected_state = state.clone();
    let label = success_label.to_string();
    let code = tokio::task::spawn_blocking(move || {
        oauth_loopback::wait_for_redirect(listener, &expected_state, "Google", &label)
    })
    .await
    .map_err(|e| Error::Other(format!("sign-in task panicked: {e}")))??;

    let mut token = exchange_code(
        &client_id,
        client_secret.expose(),
        &code,
        &redirect_uri,
        &verifier,
    )
    .await?;
    // Remember which client minted the grant, so every refresh uses that same client.
    token.client_id = Some(client_id);
    if token.refresh_token.is_none() {
        // Without offline access we can't refresh; tell the user how to fix it.
        return Err(Error::Other(
            "Google didn't grant offline access. Remove PM at myaccount.google.com/permissions, \
             then reconnect."
                .into(),
        ));
    }
    Ok(token)
}

/// GET a Google API URL as JSON, authorised with the token under `token_key`. Refreshes the
/// access token first if it's near expiry, and retries once after a refresh on 401. Never
/// touches the DB, so callers hold no lock across it (rule #4).
pub async fn authorized_get(token_key: &str, url: &str) -> Result<serde_json::Value> {
    authorized_get_with_keys(token_key, url, None).await
}

/// As [`authorized_get`], plus the Drive `X-Goog-Drive-Resource-Keys` header when `resource_keys` is
/// `Some` — required to read some LINK-shared items (a "Shared with me" file the user reached via a
/// link and hasn't opened before). The header value is `fileId/resourceKey` (comma-separated for
/// several). `None` sends no header, so every existing caller is unchanged.
pub async fn authorized_get_with_keys(
    token_key: &str,
    url: &str,
    resource_keys: Option<&str>,
) -> Result<serde_json::Value> {
    let resp = authorized_send(&http()?, token_key, |c, bearer| {
        let rb = c.get(url).bearer_auth(bearer);
        match resource_keys {
            Some(k) => rb.header("X-Goog-Drive-Resource-Keys", k),
            None => rb,
        }
    })
    .await?;
    json_or_err(resp).await
}

/// As [`authorized_get`], but returns the raw response BYTES — for Drive file downloads/exports,
/// whose bodies are not JSON. Caps the body at `max_bytes` so a huge file can't balloon memory.
pub async fn authorized_get_bytes(token_key: &str, url: &str, max_bytes: usize) -> Result<Vec<u8>> {
    authorized_get_bytes_with_keys(token_key, url, max_bytes, None).await
}

/// As [`authorized_get_bytes`], plus the `X-Goog-Drive-Resource-Keys` header (see
/// [`authorized_get_with_keys`]) for downloading/exporting a link-shared item's body.
pub async fn authorized_get_bytes_with_keys(
    token_key: &str,
    url: &str,
    max_bytes: usize,
    resource_keys: Option<&str>,
) -> Result<Vec<u8>> {
    let resp = authorized_send(&http()?, token_key, |c, bearer| {
        let rb = c.get(url).bearer_auth(bearer);
        match resource_keys {
            Some(k) => rb.header("X-Goog-Drive-Resource-Keys", k),
            None => rb,
        }
    })
    .await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let detail = crate::error::truncate_detail(&resp.text().await.unwrap_or_default());
        return Err(Error::Other(format!(
            "Google API request failed ({status}): {detail}"
        )));
    }
    read_capped_bytes(resp, max_bytes).await
}

/// One-shot authorised JSON GET using an IN-HAND token (not yet persisted) — used right after
/// consent to learn which account a fresh Drive token grants, before it is saved under that
/// account's key. No refresh (the token is seconds old).
pub async fn get_json_with_token(token: &Token, url: &str) -> Result<serde_json::Value> {
    let resp = http()?
        .get(url)
        .bearer_auth(token.access_token.expose())
        .send()
        .await?;
    json_or_err(resp).await
}

/// Send an authorised Google request built by `build`: proactive refresh a minute before expiry,
/// plus a single 401-retry-after-refresh (the backstop for a token revoked or expired early). `build`
/// is re-invoked to construct a fresh request on retry, so it must be cheap/idempotent — only small
/// metadata calls take the retry path (the backup uploader streams its big body once, with a
/// pre-refreshed token). The `client` is caller-supplied so each caller keeps its own timeout policy:
/// the default 30s for GETs, the long-transfer client for backup metadata. This is the single home of
/// Google's authorised-send-with-refresh (promoted from the backup layer's private copy so Drive's
/// REST plumbing lives once). Never touches the DB, so callers hold no lock across it (rule #4).
pub async fn authorized_send<F>(
    client: &reqwest::Client,
    token_key: &str,
    build: F,
) -> Result<reqwest::Response>
where
    F: Fn(&reqwest::Client, &str) -> reqwest::RequestBuilder,
{
    let bearer = valid_access_token(token_key).await?;
    let mut resp = build(client, bearer.expose()).send().await?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        let bearer = refresh_now(token_key).await?;
        resp = build(client, bearer.expose()).send().await?;
        // A refresh that succeeds but whose new access token is still rejected (revoked grant /
        // scope downgrade) would otherwise surface a raw provider 401 body. Map it to a clear
        // "reconnect" message instead.
        if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(Error::Other(
                "Your Google session has expired — reconnect the account in Settings → Connectors."
                    .into(),
            ));
        }
    }
    // A transient 429 throttle (Drive throttles big first-syncs harder than steady state): honour one
    // bounded `Retry-After` and retry once, mirroring the OneDrive/Graph send path so both providers
    // handle throttling in one place. The bearer is still valid — a throttle isn't an auth problem, so
    // no refresh. A 403 *usage-limit* (Drive's other throttle shape) can't be told from an auth 403
    // without reading the body, so it isn't retried here; the sync classifies it as retryable via
    // [`crate::drive::is_rate_limited`] and simply re-checks the account next pass (F-26).
    if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let wait = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(2)
            .min(60);
        tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
        let bearer = valid_access_token(token_key).await?;
        resp = build(client, bearer.expose()).send().await?;
    }
    Ok(resp)
}

/// Read a response body into bytes, but never buffer more than `max` — a huge Drive file must
/// not be able to balloon memory.
async fn read_capped_bytes(resp: reqwest::Response, max: usize) -> Result<Vec<u8>> {
    use futures_util::StreamExt;
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if buf.len() + chunk.len() > max {
            return Err(Error::Other(
                "That Google Drive file is too large to index.".into(),
            ));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

async fn json_or_err(resp: reqwest::Response) -> Result<serde_json::Value> {
    if !resp.status().is_success() {
        let status = resp.status();
        let detail = crate::error::truncate_detail(&resp.text().await.unwrap_or_default());
        return Err(Error::Other(format!(
            "Google API request failed ({status}): {detail}"
        )));
    }
    resp.json().await.map_err(Error::from)
}

// --- token exchange / refresh ---

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    scope: Option<String>,
}

async fn exchange_code(
    client_id: &str,
    client_secret: &str,
    code: &str,
    redirect_uri: &str,
    verifier: &str,
) -> Result<Token> {
    let params = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("client_secret", client_secret),
        ("code_verifier", verifier),
    ];
    let resp = http()?.post(TOKEN_ENDPOINT).form(&params).send().await?;
    token_from_response(resp).await
}

/// Refresh the access token under `token_key`; carries the existing refresh token forward when
/// Google doesn't return a new one (it usually doesn't), and re-persists the blob.
///
/// Serialized per key by the shared [`oauth_loopback::refresh_lock`], and the token blob is reloaded
/// *under* that lock: if a concurrent refresh of the same key won the race while we waited, we use its
/// freshly-persisted blob rather than a stale in-hand copy (Google doesn't rotate refresh tokens, but
/// this keeps the two providers' refresh path identical and avoids a redundant network round-trip).
/// `force = false` (the proactive path) returns early when the reloaded token is already fresh;
/// `force = true` (the reactive 401 path) always refreshes, because the token may be revoked, not
/// merely expired. The client is chosen from that reloaded blob too, under the lock: a consent or
/// [`pin_legacy_token`] that re-saved the blob while we waited may have changed which client it
/// needs, and a refresh token only works with the client that minted it.
async fn do_refresh(token_key: &str, force: bool) -> Result<Token> {
    let _guard = oauth_loopback::refresh_lock(token_key).await;
    let current = load_token(token_key)?;
    if !force && current.expiry > oauth_loopback::now_unix() + 60 {
        return Ok(current);
    }
    let refresh = current
        .refresh_token
        .clone()
        .ok_or_else(|| Error::Other("Google session expired — reconnect in Settings.".into()))?;
    let (client_id, client_secret) = client_creds_for_token(token_key, &current)?;
    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh.expose()),
        ("client_id", client_id.as_str()),
        ("client_secret", client_secret.expose()),
    ];
    let resp = http()?.post(TOKEN_ENDPOINT).form(&params).send().await?;
    let mut token = token_from_response(resp).await?;
    if token.refresh_token.is_none() {
        token.refresh_token = Some(refresh);
    }
    // Carry the granted scopes forward too, exactly as the refresh token is. Google normally echoes
    // `scope` on a refresh, but it is not obliged to — and if it ever omits it, the stored blob loses
    // the field, `token_has_scope` reads false, and an account that genuinely HOLDS the Sheets grant
    // starts being nagged to reconnect for it. A refresh can never narrow a grant, so inheriting the
    // previous value is always at least as accurate as dropping it.
    if token.scope.is_none() {
        token.scope = current.scope.clone();
    }
    // The minting client never changes across refreshes; a refresh response never carries it.
    if token.client_id.is_none() {
        token.client_id = current.client_id.clone();
    }
    save_token(token_key, &token)?;
    Ok(token)
}

async fn token_from_response(resp: reqwest::Response) -> Result<Token> {
    if !resp.status().is_success() {
        let status = resp.status();
        let detail = crate::error::truncate_detail(&resp.text().await.unwrap_or_default());
        return Err(Error::Other(format!(
            "Google sign-in failed ({status}): {detail}"
        )));
    }
    let t: TokenResponse = resp.json().await?;
    Ok(Token {
        access_token: Secret::from(t.access_token),
        refresh_token: t.refresh_token.map(Secret::from),
        expiry: oauth_loopback::now_unix() + t.expires_in.unwrap_or(3600),
        scope: t.scope,
        client_id: None,
    })
}

fn load_token(token_key: &str) -> Result<Token> {
    let raw = secrets::get_google_token_for(token_key)?
        .ok_or_else(|| Error::Other("Not connected to Google. Connect in Settings.".into()))?;
    serde_json::from_str(raw.expose())
        .map_err(|e| Error::Other(format!("stored Google token unreadable: {e}")))
}

/// Pin a token saved before minting clients were recorded to the client it is refreshing through
/// NOW — by the same legacy rule [`client_creds_for_token`] applies to its key — so that saving or
/// replacing an own client for the account next can't redirect it to a client that never minted it.
/// A token that already names its client, or no token at all, is left alone. Taken under the key's
/// refresh lock so it can't race a refresh that re-saves the blob. Call it BEFORE the client changes.
pub async fn pin_legacy_token(token_key: &str) -> Result<()> {
    let _guard = oauth_loopback::refresh_lock(token_key).await;
    let Some(raw) = secrets::get_google_token_for(token_key)? else {
        return Ok(());
    };
    let mut token: Token = serde_json::from_str(raw.expose())
        .map_err(|e| Error::Other(format!("stored Google token unreadable: {e}")))?;
    if token.client_id.is_some() {
        return Ok(());
    }
    // No client resolves (none saved at all): the token can't refresh either way, so leave it be.
    let Ok((client_id, _)) = client_creds_for_token(token_key, &token) else {
        return Ok(());
    };
    token.client_id = Some(client_id);
    save_token(token_key, &token)
}

/// Persist a token blob under its service/account keychain key. Public so a connector can save
/// the token returned by [`run_consent`] once it knows which account/key it belongs to.
pub fn save_token(token_key: &str, token: &Token) -> Result<()> {
    let json = serde_json::to_string(token).map_err(|e| Error::Other(e.to_string()))?;
    secrets::set_google_token_for(token_key, &json)
}

/// Save a token a consent just returned, under the key's refresh lock. A refresh already running for
/// the key (a sync in flight while the browser was open) loaded the OLD blob and re-saves it when its
/// round-trip ends; saved outside the lock, the new token could land in between and be overwritten by
/// a refreshed copy of the old one — losing, say, the calendar write scope the user just granted.
/// Under the lock the save waits for that refresh, and the next one reloads the new blob.
pub async fn save_consented_token(token_key: &str, token: &Token) -> Result<()> {
    let _guard = oauth_loopback::refresh_lock(token_key).await;
    save_token(token_key, token)
}

/// A currently-valid bearer access token for `token_key`, refreshing proactively if it's within
/// 60s of expiry (and re-persisting the refreshed blob). The shared [`authorized_send`] uses this for
/// its proactive refresh; it's also public so callers that stream their OWN request — the backup
/// uploader's chunked Drive PUT, sent once outside the retry helper — can authorize it and handle a
/// reactive 401 via [`refresh_now`].
pub async fn valid_access_token(token_key: &str) -> Result<Secret> {
    let mut token = load_token(token_key)?;
    // Lock-free fast path; `do_refresh` re-checks expiry under the per-key lock before any network call.
    if token.expiry <= oauth_loopback::now_unix() + 60 {
        token = do_refresh(token_key, false).await?;
    }
    Ok(token.access_token.clone())
}

/// Force a token refresh for `token_key` and return the new bearer — the backstop for a token
/// revoked or expired early (a 401 on a request built with [`valid_access_token`]). Re-persists
/// the refreshed blob, exactly like the GET path's reactive refresh.
pub async fn refresh_now(token_key: &str) -> Result<Secret> {
    let refreshed = do_refresh(token_key, true).await?;
    Ok(refreshed.access_token.clone())
}

/// Whether the stored token for `token_key` already carries `scope` in its granted set. Lets the
/// backup layer tell — from the keychain, no network — whether an account has the `drive.file`
/// write grant yet (so the scheduler skips a Drive push whose grant was never given or was
/// revoked). A missing token or missing `scope` field reads as "no".
pub fn token_has_scope(token_key: &str, scope: &str) -> Result<bool> {
    Ok(token_scope(token_key)?.is_some_and(|granted| scope_set_has(&granted, scope)))
}

/// Dev builds only: what the stored token for `token_key` was granted, for the grant report a live
/// test reads (the `dev_google_grant_report` command). Scopes without Google's URL prefix, and
/// which kind of client minted it; never a token, a client id or a secret. `None` with no token.
#[cfg(debug_assertions)]
pub fn grant_summary(token_key: &str) -> Result<Option<(Vec<String>, &'static str)>> {
    let Some(raw) = secrets::get_google_token_for(token_key)? else {
        return Ok(None);
    };
    let token: Token = serde_json::from_str(raw.expose())
        .map_err(|e| Error::Other(format!("stored Google token unreadable: {e}")))?;
    let scopes = token
        .scope
        .as_deref()
        .unwrap_or_default()
        .split_ascii_whitespace()
        .map(|s| {
            s.trim_start_matches("https://www.googleapis.com/auth/")
                .to_string()
        })
        .collect();
    let own = match token_key.rsplit_once("::") {
        Some((_, email)) => secrets::get_google_client_for_account(email)?.map(|(id, _)| id),
        None => None,
    };
    let shared = secrets::get_google_client_id()?;
    let client = match (
        token.client_id.as_deref(),
        choose_client(
            token.client_id.as_deref(),
            own.as_deref(),
            shared.as_deref(),
        ),
    ) {
        (None, _) => "unrecorded",
        (Some(_), ClientChoice::Own) => "own",
        (Some(_), ClientChoice::Shared) => "shared",
        (Some(_), ClientChoice::Gone) => "gone",
    };
    Ok(Some((scopes, client)))
}

/// The granted scope set of the stored token for `token_key`, or `None` when there's no token or it
/// never recorded one. Read from the keychain, no network.
pub fn token_scope(token_key: &str) -> Result<Option<String>> {
    let Some(raw) = secrets::get_google_token_for(token_key)? else {
        return Ok(None);
    };
    let token: Token = serde_json::from_str(raw.expose())
        .map_err(|e| Error::Other(format!("stored Google token unreadable: {e}")))?;
    Ok(token.scope)
}

// --- auth URL (PKCE + loopback machinery live in `crate::oauth_loopback`) ---

/// Build Google's consent URL. `access_type=offline` + `prompt=consent` guarantee a
/// refresh token so PM can stay connected. `select_account` forces Google's account chooser every
/// time, so connecting a *second* account actually works — without it, Google silently reuses the
/// browser's signed-in session and re-grants the same account, which is why "Add another account"
/// could only ever re-link the first one. `login_hint` (an account PM already knows) starts the
/// chooser on that account; the user can still pick another. `include_granted_scopes` follows the
/// scope set ([`merges_granted_scopes`]), decided here so no consent can ask otherwise. Pure, so it's
/// unit-tested.
pub fn build_auth_url(
    client_id: &str,
    redirect_uri: &str,
    challenge: &str,
    state: &str,
    scope: &str,
    login_hint: Option<&str>,
) -> Result<String> {
    let include_granted = merges_granted_scopes(scope);
    let mut params = vec![
        ("client_id", client_id),
        ("redirect_uri", redirect_uri),
        ("response_type", "code"),
        ("scope", scope),
        ("code_challenge", challenge),
        ("code_challenge_method", "S256"),
        ("state", state),
        ("access_type", "offline"),
        ("prompt", "select_account consent"),
        (
            "include_granted_scopes",
            if include_granted { "true" } else { "false" },
        ),
    ];
    if let Some(hint) = login_hint {
        params.push(("login_hint", hint));
    }
    let url = reqwest::Url::parse_with_params(AUTH_ENDPOINT, &params)
        .map_err(|e| Error::Other(format!("could not build auth URL: {e}")))?;
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_url_carries_pkce_offline_and_readonly_scope() {
        let url = build_auth_url(
            "client-123",
            "http://127.0.0.1:54321",
            "chal",
            "state-abc",
            CALENDAR_SCOPE,
            None,
        )
        .unwrap();
        assert!(url.starts_with(AUTH_ENDPOINT));
        assert!(!url.contains("login_hint"));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("access_type=offline"));
        // The account chooser is forced (space-joined prompt values url-encode the space as `+`),
        // so a second Google account can actually be connected instead of silently re-linking the
        // browser's current session.
        assert!(url.contains("prompt=select_account+consent"));
        assert!(url.contains("calendar.readonly"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A54321"));
        assert!(url.contains("state=state-abc"));
    }

    #[test]
    fn a_known_account_is_passed_as_the_login_hint() {
        let url = build_auth_url(
            "client-123",
            "http://127.0.0.1:54321",
            "chal",
            "state-abc",
            DRIVE_FILE_SCOPE,
            Some("ap@example.com"),
        )
        .unwrap();
        assert!(url.contains("login_hint=ap%40example.com"));
        // The chooser stays forced: the hint only picks where it starts.
        assert!(url.contains("prompt=select_account+consent"));
    }

    /// Calendar consents state their whole scope set and never fold in earlier grants, so a read-only
    /// reconnect can't come back holding the write scope; Drive and backup consents still merge, which
    /// is how a backup's `drive.file` joins the Drive connector's read scopes on one token.
    #[test]
    fn only_calendar_consents_stop_merging_earlier_grants() {
        let editing = calendar_editing_scopes();
        let drive = format!("{DRIVE_SCOPE} {SHEETS_SCOPE}");
        for (scope, merges) in [
            (CALENDAR_SCOPE, false),
            (editing.as_str(), false),
            (drive.as_str(), true),
            (DRIVE_FILE_SCOPE, true),
        ] {
            assert_eq!(merges_granted_scopes(scope), merges, "{scope}");
            // The URL every consent opens carries it: the builder derives it from the scope.
            let url = build_auth_url("c", "http://127.0.0.1:1", "x", "s", scope, None).unwrap();
            assert!(
                url.contains(&format!("include_granted_scopes={merges}")),
                "{url}"
            );
        }
        // The editing consent asks for the read scope by name, since nothing merges it in.
        assert!(scope_set_has(&editing, CALENDAR_SCOPE));
        assert!(scope_set_has(&editing, CALENDAR_EVENTS_SCOPE));
    }

    #[test]
    fn a_scope_set_matches_whole_scopes_only() {
        let granted = format!("openid {CALENDAR_SCOPE}  {DRIVE_FILE_SCOPE}");
        assert!(scope_set_has(&granted, CALENDAR_SCOPE));
        assert!(scope_set_has(&granted, DRIVE_FILE_SCOPE));
        assert!(!scope_set_has(&granted, CALENDAR_EVENTS_SCOPE));
        // A prefix of a granted scope is not that scope.
        assert!(!scope_set_has(
            &granted,
            "https://www.googleapis.com/auth/calendar"
        ));
        assert!(!scope_set_has("", CALENDAR_SCOPE));
    }

    /// A refresh token only works with the client that minted it, so a token that records its client
    /// refreshes through exactly that one; a token saved before ids were recorded keeps the old rule.
    #[test]
    fn a_token_refreshes_through_the_client_that_minted_it() {
        use ClientChoice::{Gone, Own, Shared};
        let cases = [
            // (minted_by, own saved, shared saved, expected)
            (Some("own-1"), Some("own-1"), Some("shared-1"), Own),
            // The case the old rule got wrong: an account with its own client saved, whose token
            // was minted by the shared client (connected through the normal button).
            (Some("shared-1"), Some("own-1"), Some("shared-1"), Shared),
            (Some("shared-1"), None, Some("shared-1"), Shared),
            // The minting client was cleared or replaced: say so rather than refresh with another.
            (Some("old-own"), Some("own-2"), Some("shared-1"), Gone),
            (Some("shared-old"), None, Some("shared-new"), Gone),
            (Some("own-1"), None, None, Gone),
            // Tokens from before ids were recorded.
            (None, Some("own-1"), Some("shared-1"), Own),
            (None, None, Some("shared-1"), Shared),
            (None, None, None, Shared),
        ];
        for (minted_by, own, shared, want) in cases {
            assert_eq!(
                choose_client(minted_by, own, shared),
                want,
                "minted_by={minted_by:?} own={own:?} shared={shared:?}"
            );
        }
    }

    /// The keychain blob of a token saved before 3.139.9 has no `client_id`; it must still load, and a
    /// token without one must save byte-for-byte as before.
    #[test]
    fn the_token_blob_round_trips_with_and_without_a_minting_client() {
        let old = r#"{"access_token":"a","refresh_token":"r","expiry":1,"scope":"s"}"#;
        let token: Token = serde_json::from_str(old).unwrap();
        assert_eq!(token.client_id, None);
        assert_eq!(serde_json::to_string(&token).unwrap(), old);

        let mut minted = token.clone();
        minted.client_id = Some("own-1".into());
        let json = serde_json::to_string(&minted).unwrap();
        assert!(json.ends_with(r#","client_id":"own-1"}"#), "{json}");
        let back: Token = serde_json::from_str(&json).unwrap();
        assert_eq!(back.client_id.as_deref(), Some("own-1"));
    }
}
