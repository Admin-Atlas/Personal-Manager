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

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
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
/// How long to wait for the browser consent redirect before giving up.
const REDIRECT_TIMEOUT_SECS: u64 = 180;

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
    let (verifier, challenge) = pkce()?;
    let state = random_token(16)?;

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
    let code =
        tokio::task::spawn_blocking(move || wait_for_redirect(listener, &expected_state, &label))
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
    let resp = authorized_send(token_key, url).await?;
    json_or_err(resp).await
}

/// As [`authorized_get`], but returns the raw response BYTES — for Drive file downloads/exports,
/// whose bodies are not JSON. Caps the body at `max_bytes` so a huge file can't balloon memory.
pub async fn authorized_get_bytes(token_key: &str, url: &str, max_bytes: usize) -> Result<Vec<u8>> {
    let resp = authorized_send(token_key, url).await?;
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

/// Shared authorised GET: proactive refresh a minute before expiry, plus a single
/// 401-retry-after-refresh (the backstop for a token revoked or expired early). Returns the raw
/// response so the caller can decode it as JSON or bytes.
async fn authorized_send(token_key: &str, url: &str) -> Result<reqwest::Response> {
    let (client_id, client_secret) = client_creds_for_key(token_key)?;
    let mut token = load_token(token_key)?;

    if token.expiry <= now_unix() + 60 {
        token = do_refresh(&client_id, client_secret.expose(), &token, token_key).await?;
    }

    let resp = http()?
        .get(url)
        .bearer_auth(token.access_token.expose())
        .send()
        .await?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        let token = do_refresh(&client_id, client_secret.expose(), &token, token_key).await?;
        let retried = http()?
            .get(url)
            .bearer_auth(token.access_token.expose())
            .send()
            .await?;
        // A refresh that succeeds but whose new access token is still rejected (revoked grant /
        // scope downgrade) would otherwise surface a raw provider 401 body. Map it to a clear
        // "reconnect" message instead.
        if retried.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err(Error::Other(
                "Your Google session has expired — reconnect the account in Settings → Connectors."
                    .into(),
            ));
        }
        return Ok(retried);
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
async fn do_refresh(
    client_id: &str,
    client_secret: &str,
    current: &Token,
    token_key: &str,
) -> Result<Token> {
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
        expiry: now_unix() + t.expires_in.unwrap_or(3600),
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
/// 60s of expiry (and re-persisting the refreshed blob). Public so callers that build their OWN
/// requests — the backup uploader's Drive POST/PUT/PATCH — can authorize them; the private
/// [`authorized_send`] is GET-only. Callers should still handle a reactive 401 via [`refresh_now`].
pub async fn valid_access_token(token_key: &str) -> Result<Secret> {
    let (client_id, client_secret) = client_creds_for_key(token_key)?;
    let mut token = load_token(token_key)?;
    if token.expiry <= now_unix() + 60 {
        token = do_refresh(&client_id, client_secret.expose(), &token, token_key).await?;
    }
    Ok(token.access_token.clone())
}

/// Force a token refresh for `token_key` and return the new bearer — the backstop for a token
/// revoked or expired early (a 401 on a request built with [`valid_access_token`]). Re-persists
/// the refreshed blob, exactly like the GET path's reactive refresh.
pub async fn refresh_now(token_key: &str) -> Result<Secret> {
    let (client_id, client_secret) = client_creds_for_key(token_key)?;
    let token = load_token(token_key)?;
    let refreshed = do_refresh(&client_id, client_secret.expose(), &token, token_key).await?;
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

// --- PKCE + auth URL ---

/// A PKCE verifier/challenge pair. The verifier is hex (a valid PKCE charset); the
/// challenge is the base64url-nopad SHA-256 of the verifier (the S256 method).
fn pkce() -> Result<(String, String)> {
    let verifier = random_token(32)?;
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    Ok((verifier, challenge))
}

/// `n` random bytes, hex-encoded — used for the PKCE verifier and the CSRF state.
fn random_token(n: usize) -> Result<String> {
    let mut bytes = vec![0u8; n];
    getrandom::fill(&mut bytes).map_err(|e| Error::Other(format!("rng failure: {e}")))?;
    Ok(hex::encode(bytes))
}

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

// --- loopback redirect server ---

/// Accept connections until the OAuth redirect arrives, validate the CSRF `state`,
/// and return the authorization `code`. Polls with a deadline so it can't hang
/// forever (browsers also make stray requests like /favicon.ico, which we ignore).
fn wait_for_redirect(
    listener: std::net::TcpListener,
    expected_state: &str,
    success_label: &str,
) -> Result<String> {
    use std::io::Read;

    listener.set_nonblocking(true)?;
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(REDIRECT_TIMEOUT_SECS);

    loop {
        if std::time::Instant::now() >= deadline {
            return Err(Error::Other(
                "Timed out waiting for Google sign-in. Please try again.".into(),
            ));
        }
        let (mut stream, _) = match listener.accept() {
            Ok(pair) => pair,
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(150));
                continue;
            }
            Err(e) => return Err(e.into()),
        };

        stream.set_nonblocking(false).ok();
        stream
            .set_read_timeout(Some(std::time::Duration::from_secs(5)))
            .ok();
        let mut buf = [0u8; 4096];
        let n = match stream.read(&mut buf) {
            Ok(n) => n,
            Err(_) => continue,
        };
        let request = String::from_utf8_lossy(&buf[..n]);
        let Some(target) = request_target(&request) else {
            let _ = write_page(&mut stream, "Waiting for Google…");
            continue;
        };
        let params = parse_query(&target);

        if let Some(err) = params.get("error") {
            let _ = write_page(
                &mut stream,
                "Sign-in was cancelled. You can close this tab.",
            );
            return Err(Error::Other(format!("Google sign-in was declined: {err}")));
        }
        match params.get("code") {
            Some(code)
                if params
                    .get("state")
                    .is_some_and(|s| ct_eq(s, expected_state)) =>
            {
                let _ = write_page(
                    &mut stream,
                    &format!(
                        "PM is connected to {success_label}. You can close this tab and return to PM."
                    ),
                );
                return Ok(code.clone());
            }
            Some(_) => {
                let _ = write_page(
                    &mut stream,
                    "Sign-in could not be verified. Please try again.",
                );
                return Err(Error::Other(
                    "OAuth state mismatch — sign-in aborted for safety.".into(),
                ));
            }
            None => {
                // Not the redirect (e.g. a favicon probe) — keep waiting.
                let _ = write_page(&mut stream, "Waiting for Google…");
            }
        }
    }
}

/// The request target from the first request line: `GET /?code=… HTTP/1.1` → `/?code=…`.
fn request_target(request: &str) -> Option<String> {
    let line = request.lines().next()?;
    line.split_whitespace().nth(1).map(str::to_string)
}

/// Constant-time equality for the OAuth CSRF `state` token, so the comparison
/// can't be turned into a timing oracle. (The length check is fine to short-circuit
/// — the token length is fixed and not secret.)
fn ct_eq(a: &str, b: &str) -> bool {
    let (a, b) = (a.as_bytes(), b.as_bytes());
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// Parse a `/path?a=1&b=2` target's query into a map, percent-decoding values.
fn parse_query(target: &str) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();
    if let Some((_, query)) = target.split_once('?') {
        for pair in query.split('&') {
            if let Some((k, v)) = pair.split_once('=') {
                map.insert(k.to_string(), percent_decode(v));
            }
        }
    }
    map
}

/// Minimal application/x-www-form-urlencoded decode (handles `+` and `%XX`).
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                let hi = (bytes[i + 1] as char).to_digit(16);
                let lo = (bytes[i + 2] as char).to_digit(16);
                if let (Some(hi), Some(lo)) = (hi, lo) {
                    out.push((hi * 16 + lo) as u8);
                    i += 3;
                    continue;
                }
                out.push(b'%');
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Write a tiny styled HTML page back to the browser and close the connection.
fn write_page(stream: &mut std::net::TcpStream, message: &str) -> std::io::Result<()> {
    use std::io::Write;
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>PM</title></head>\
         <body style=\"font-family:system-ui,sans-serif;background:#0a0a0a;color:#e5e5e5;\
         display:flex;align-items:center;justify-content:center;height:100vh;margin:0\">\
         <p style=\"font-size:15px;max-width:28rem;text-align:center;padding:0 1rem\">{message}</p>\
         </body></html>"
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes())
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pkce_challenge_matches_known_vector() {
        // RFC 7636 Appendix B reference verifier → challenge.
        let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
        let digest = Sha256::digest(verifier.as_bytes());
        let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
        assert_eq!(challenge, "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
    }

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

    #[test]
    fn query_parsing_decodes_and_extracts_code() {
        let params = parse_query("/?state=abc&code=4%2F0Ab%2Cd&scope=read");
        assert_eq!(params.get("code").unwrap(), "4/0Ab,d");
        assert_eq!(params.get("state").unwrap(), "abc");
    }

    #[test]
    fn request_target_pulls_the_path() {
        let req = "GET /?code=xyz&state=s HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
        assert_eq!(request_target(req).unwrap(), "/?code=xyz&state=s");
    }
}
