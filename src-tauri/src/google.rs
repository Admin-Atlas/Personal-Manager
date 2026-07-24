// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Google OAuth — PM's first connector (spec §8.6). BYO credentials: the user
//! supplies a Google Cloud "Desktop app" OAuth client (id + secret), so no Google
//! secret ships in the repo (rule #1). The flow is the recommended desktop pattern:
//! a **loopback redirect with PKCE** — PM opens the system browser to Google's
//! consent screen with `redirect_uri=http://127.0.0.1:<ephemeral-port>`, runs a
//! one-shot local HTTP server to catch the redirect, and exchanges the code for
//! tokens. Scopes are **read-only** (spec non-goal #4). Access tokens are refreshed
//! transparently; the token blob lives only in the keychain, never on disk.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::oauth_loopback;
use crate::secret::Secret;
use crate::secrets;

const AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
/// Read-only calendar scope — PM reads events, never writes (spec non-goal #4).
pub const CALENDAR_SCOPE: &str = "https://www.googleapis.com/auth/calendar.readonly";
/// Read-only Drive scope — PM reads file metadata + content, never writes. The full-read
/// `drive.readonly` (not `drive.metadata.readonly`) because index-only ingestion needs each
/// file's body to embed it.
pub const DRIVE_SCOPE: &str = "https://www.googleapis.com/auth/drive.readonly";
/// Read-only Sheets scope — requested ALONGSIDE `drive.readonly` when connecting a Drive account, so
/// PM can read a Google Sheet's tab names + header row via the Sheets API for the metadata-only Sheets
/// index (never the full grid). Because a refresh token cannot broaden its grant, adding this scope
/// means every EXISTING Drive account must re-consent to gain it; PM detects who needs it — offline,
/// no network — with [`token_has_scope`] and surfaces a per-account "Reconnect for Sheets" prompt.
/// `build_auth_url`'s `include_granted_scopes=true` unions it onto the account's existing Drive grant.
pub const SHEETS_SCOPE: &str = "https://www.googleapis.com/auth/spreadsheets.readonly";
/// The ONLY Google **write** scope PM ever requests — least-privilege, granted just for
/// encrypted backup. `drive.file` can create and manage only files/folders the app itself
/// created (PM's "Personal Manager Backups" folder and its `.pmbackup` archives); it can never
/// touch the user's other Drive content. Requested via a dedicated re-consent (the connector
/// scopes are read-only), which UNIONS it with any existing `drive.readonly` grant on the account
/// because `build_auth_url` sets `include_granted_scopes=true`.
pub const DRIVE_FILE_SCOPE: &str = "https://www.googleapis.com/auth/drive.file";
/// The keychain key for the Calendar service's token — passed into the per-service token
/// helpers below (the connector-generic flow takes a key so Drive accounts get their own).
pub const CALENDAR_TOKEN_KEY: &str = secrets::GOOGLE_TOKEN_CALENDAR;

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

/// The OAuth client to use for the account behind a token key — its OWN client if one is stored
/// (an Advanced-Protection account on its own Cloud project), else the shared client. The account
/// email is the suffix after `::` in the token key (`google_oauth_token_drive::<email>` etc.); the
/// legacy fixed calendar key has no suffix, so it resolves to the shared client.
fn client_creds_for_key(token_key: &str) -> Result<(String, Secret)> {
    if let Some((_, email)) = token_key.rsplit_once("::") {
        if let (Some(id), Some(secret)) = (
            secrets::get_google_client_id_for_account(email)?,
            secrets::get_google_client_secret_for_account(email)?,
        ) {
            return Ok((id, secret));
        }
    }
    client_creds()
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
    run_consent_inner(scope, success_label, client_creds()?).await
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
    )
    .await
}

async fn run_consent_inner(
    scope: &str,
    success_label: &str,
    (client_id, client_secret): (String, Secret),
) -> Result<Token> {
    let (verifier, challenge) = oauth_loopback::pkce()?;
    let state = oauth_loopback::random_token(16)?;

    // Bind the loopback listener first so the port is known before we build the URL.
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| Error::Other(format!("Could not start the local sign-in server: {e}")))?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}");

    let auth_url = build_auth_url(&client_id, &redirect_uri, &challenge, &state, scope)?;
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

    let token = exchange_code(
        &client_id,
        client_secret.expose(),
        &code,
        &redirect_uri,
        &verifier,
    )
    .await?;
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
/// merely expired.
async fn do_refresh(
    client_id: &str,
    client_secret: &str,
    token_key: &str,
    force: bool,
) -> Result<Token> {
    let _guard = oauth_loopback::refresh_lock(token_key).await;
    let current = load_token(token_key)?;
    if !force && current.expiry > oauth_loopback::now_unix() + 60 {
        return Ok(current);
    }
    let refresh = current
        .refresh_token
        .clone()
        .ok_or_else(|| Error::Other("Google session expired — reconnect in Settings.".into()))?;
    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh.expose()),
        ("client_id", client_id),
        ("client_secret", client_secret),
    ];
    let resp = http()?.post(TOKEN_ENDPOINT).form(&params).send().await?;
    let mut token = token_from_response(resp).await?;
    if token.refresh_token.is_none() {
        token.refresh_token = Some(refresh);
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
    })
}

fn load_token(token_key: &str) -> Result<Token> {
    let raw = secrets::get_google_token_for(token_key)?
        .ok_or_else(|| Error::Other("Not connected to Google. Connect in Settings.".into()))?;
    serde_json::from_str(raw.expose())
        .map_err(|e| Error::Other(format!("stored Google token unreadable: {e}")))
}

/// Persist a token blob under its service/account keychain key. Public so a connector can save
/// the token returned by [`run_consent`] once it knows which account/key it belongs to.
pub fn save_token(token_key: &str, token: &Token) -> Result<()> {
    let json = serde_json::to_string(token).map_err(|e| Error::Other(e.to_string()))?;
    secrets::set_google_token_for(token_key, &json)
}

/// A currently-valid bearer access token for `token_key`, refreshing proactively if it's within
/// 60s of expiry (and re-persisting the refreshed blob). The shared [`authorized_send`] uses this for
/// its proactive refresh; it's also public so callers that stream their OWN request — the backup
/// uploader's chunked Drive PUT, sent once outside the retry helper — can authorize it and handle a
/// reactive 401 via [`refresh_now`].
pub async fn valid_access_token(token_key: &str) -> Result<Secret> {
    let (client_id, client_secret) = client_creds_for_key(token_key)?;
    let mut token = load_token(token_key)?;
    // Lock-free fast path; `do_refresh` re-checks expiry under the per-key lock before any network call.
    if token.expiry <= oauth_loopback::now_unix() + 60 {
        token = do_refresh(&client_id, client_secret.expose(), token_key, false).await?;
    }
    Ok(token.access_token.clone())
}

/// Force a token refresh for `token_key` and return the new bearer — the backstop for a token
/// revoked or expired early (a 401 on a request built with [`valid_access_token`]). Re-persists
/// the refreshed blob, exactly like the GET path's reactive refresh.
pub async fn refresh_now(token_key: &str) -> Result<Secret> {
    let (client_id, client_secret) = client_creds_for_key(token_key)?;
    let refreshed = do_refresh(&client_id, client_secret.expose(), token_key, true).await?;
    Ok(refreshed.access_token.clone())
}

/// Whether the stored token for `token_key` already carries `scope` in its granted set. Lets the
/// backup layer tell — from the keychain, no network — whether an account has the `drive.file`
/// write grant yet (so the scheduler skips a Drive push whose grant was never given or was
/// revoked). A missing token or missing `scope` field reads as "no".
pub fn token_has_scope(token_key: &str, scope: &str) -> Result<bool> {
    let Some(raw) = secrets::get_google_token_for(token_key)? else {
        return Ok(false);
    };
    let token: Token = serde_json::from_str(raw.expose())
        .map_err(|e| Error::Other(format!("stored Google token unreadable: {e}")))?;
    Ok(token
        .scope
        .as_deref()
        .map(|s| s.split(' ').any(|granted| granted == scope))
        .unwrap_or(false))
}

// --- auth URL (PKCE + loopback machinery live in `crate::oauth_loopback`) ---

/// Build Google's consent URL. `access_type=offline` + `prompt=consent` guarantee a
/// refresh token so PM can stay connected. `select_account` forces Google's account chooser every
/// time, so connecting a *second* account actually works — without it, Google silently reuses the
/// browser's signed-in session and re-grants the same account, which is why "Add another account"
/// could only ever re-link the first one. Pure, so it's unit-tested.
pub fn build_auth_url(
    client_id: &str,
    redirect_uri: &str,
    challenge: &str,
    state: &str,
    scope: &str,
) -> Result<String> {
    let url = reqwest::Url::parse_with_params(
        AUTH_ENDPOINT,
        &[
            ("client_id", client_id),
            ("redirect_uri", redirect_uri),
            ("response_type", "code"),
            ("scope", scope),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256"),
            ("state", state),
            ("access_type", "offline"),
            ("prompt", "select_account consent"),
            ("include_granted_scopes", "true"),
        ],
    )
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
        )
        .unwrap();
        assert!(url.starts_with(AUTH_ENDPOINT));
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
}
