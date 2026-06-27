// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Microsoft (Entra ID) OAuth — the second connector provider (board card 4B, OneDrive). The
//! Microsoft sibling of [`crate::google`], and deliberately a near-mirror of it: the same loopback +
//! PKCE desktop flow, the same proactive-refresh / 401-retry token handling, tokens in the keychain
//! only. Two differences make it its own module rather than a shared one:
//!
//! 1. **Public client, no secret.** A Microsoft "Mobile & desktop" app registration is a PUBLIC
//!    client — the code exchange and refresh send only the client id, never a secret (there is none
//!    to ship, so rule #1 holds for free). The user pastes just a client id.
//! 2. **`/common` authority + `offline_access` scope.** PM signs in against the `/common` endpoint so
//!    BOTH personal Microsoft accounts and work/school accounts work, and asks for `offline_access`
//!    (rather than Google's `access_type=offline`) to get a refresh token.
//!
//! Scopes are **read-only** (`Files.Read`); PM never writes to OneDrive (spec non-goal #4).

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::secret::Secret;
use crate::secrets;

/// `/common` so both personal Microsoft accounts (consumers) and work/school accounts (orgs) can
/// sign in; the user's app registration must allow both account types for this to resolve.
const AUTH_ENDPOINT: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/authorize";
const TOKEN_ENDPOINT: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/token";
/// Microsoft Graph base for the OneDrive connector's API calls (drive items, delta, content, /me).
pub const GRAPH_API: &str = "https://graph.microsoft.com/v1.0";
/// Read-only OneDrive scope + `offline_access` (for the refresh token) + `User.Read` (to learn which
/// account the token grants, the Graph equivalent of Drive's `about`). Space-separated, as Graph
/// expects. `Files.Read` covers reading file metadata AND content (index-only needs each body to
/// embed it).
pub const ONEDRIVE_SCOPE: &str = "Files.Read offline_access User.Read";
/// How long to wait for the browser consent redirect before giving up.
const REDIRECT_TIMEOUT_SECS: u64 = 180;

/// The stored OAuth token blob (one keychain entry, JSON). `expiry` is Unix seconds. The
/// bearer/refresh values are [`Secret`], so the derived `Debug` can never print them.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Token {
    pub access_token: Secret,
    #[serde(default)]
    pub refresh_token: Option<Secret>,
    pub expiry: i64,
    #[serde(default)]
    pub scope: Option<String>,
}

/// True once the user has pasted a Microsoft client id (the public client — no secret).
pub fn has_client() -> Result<bool> {
    Ok(secrets::get_microsoft_client_id()?.is_some())
}

fn client_id() -> Result<String> {
    secrets::get_microsoft_client_id()?
        .ok_or_else(|| Error::Other("Add your Microsoft client ID in Settings first.".into()))
}

fn http() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(Error::from)
}

/// Run the OAuth consent flow and RETURN the token without persisting it: open the browser to
/// Microsoft's consent screen for `scope`, catch the loopback redirect, and exchange the code. The
/// caller chooses which keychain key to store it under — for a OneDrive account that key is derived
/// from the account the token grants (known only after a follow-up `/me` call), so persisting is the
/// caller's job. `success_label` names the connected product on the browser success page.
pub async fn run_consent(scope: &str, success_label: &str) -> Result<Token> {
    let client_id = client_id()?;
    let (verifier, challenge) = pkce()?;
    let state = random_token(16)?;

    // Bind the loopback listener first so the port is known before we build the URL. Bind 127.0.0.1
    // (not "localhost") and use it verbatim as the redirect host, so there is no DNS/IPv6 ambiguity
    // about which address the browser connects back to (RFC 8252 §7.3). The user's app registration
    // must list `http://127.0.0.1` as a redirect URI (Entra ignores the dynamic port for native
    // loopback clients).
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| Error::Other(format!("Could not start the local sign-in server: {e}")))?;
    let port = listener.local_addr()?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}");

    let auth_url = build_auth_url(&client_id, &redirect_uri, &challenge, &state, scope)?;
    open::that(&auth_url)
        .map_err(|e| Error::Other(format!("Couldn't open your browser to sign in: {e}")))?;

    // Wait for Microsoft to redirect back with the code (blocking accept, off-runtime).
    let expected_state = state.clone();
    let label = success_label.to_string();
    let code =
        tokio::task::spawn_blocking(move || wait_for_redirect(listener, &expected_state, &label))
            .await
            .map_err(|e| Error::Other(format!("sign-in task panicked: {e}")))??;

    let token = exchange_code(&client_id, &code, &redirect_uri, &verifier).await?;
    if token.refresh_token.is_none() {
        // Without offline access we can't refresh; tell the user how to fix it.
        return Err(Error::Other(
            "Microsoft didn't grant offline access. Disconnect and reconnect, and make sure the \
             consent included staying signed in."
                .into(),
        ));
    }
    Ok(token)
}

/// GET a Microsoft Graph URL as JSON, authorised with the token under `token_key`. Refreshes the
/// access token first if it's near expiry, retries once after a refresh on 401, and honours one
/// `Retry-After` on a 429 throttle. Never touches the DB, so callers hold no lock across it (rule #4).
pub async fn authorized_get(token_key: &str, url: &str) -> Result<serde_json::Value> {
    let resp = authorized_send(token_key, url).await?;
    json_or_err(resp).await
}

/// As [`authorized_get`], but returns the raw response BYTES — for OneDrive file content downloads,
/// whose bodies are not JSON. Caps the body at `max_bytes` so a huge file can't balloon memory. The
/// `/content` endpoint 302-redirects to a short-lived pre-authenticated download URL on another host;
/// reqwest follows the redirect and (correctly) drops the bearer for the cross-host hop.
pub async fn authorized_get_bytes(token_key: &str, url: &str, max_bytes: usize) -> Result<Vec<u8>> {
    let resp = authorized_send(token_key, url).await?;
    if !resp.status().is_success() {
        let status = resp.status();
        let detail = crate::error::truncate_detail(&resp.text().await.unwrap_or_default());
        return Err(Error::Other(format!(
            "Microsoft Graph request failed ({status}): {detail}"
        )));
    }
    read_capped_bytes(resp, max_bytes).await
}

/// One-shot authorised JSON GET using an IN-HAND token (not yet persisted) — used right after consent
/// to learn which account a fresh token grants (`/me`), before it is saved under that account's key.
/// No refresh (the token is seconds old).
pub async fn get_json_with_token(token: &Token, url: &str) -> Result<serde_json::Value> {
    let resp = http()?
        .get(url)
        .bearer_auth(token.access_token.expose())
        .send()
        .await?;
    json_or_err(resp).await
}

/// The account a fresh token grants (email + display name), via Graph `/me`. Personal accounts often
/// expose only `userPrincipalName`, so fall back to it when `mail` is absent; the email carries no
/// `:`, so it splits cleanly as the OneDrive source-id account segment.
pub async fn me(token: &Token) -> Result<(String, String)> {
    let v = get_json_with_token(
        token,
        &format!("{GRAPH_API}/me?$select=displayName,mail,userPrincipalName"),
    )
    .await?;
    let email = v
        .get("mail")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            v.get("userPrincipalName")
                .and_then(serde_json::Value::as_str)
        })
        .ok_or_else(|| Error::Other("Microsoft didn't return the account email.".into()))?
        .to_string();
    let name = v
        .get("displayName")
        .and_then(serde_json::Value::as_str)
        .filter(|s| !s.is_empty())
        .unwrap_or(&email)
        .to_string();
    Ok((email, name))
}

/// Shared authorised GET: proactive refresh a minute before expiry, a single 401-retry-after-refresh
/// (the backstop for a token revoked or expired early), and a single bounded `Retry-After` retry on a
/// 429 throttle (Graph throttles initial enumerations harder than Drive). Returns the raw response so
/// the caller can decode it as JSON or bytes.
async fn authorized_send(token_key: &str, url: &str) -> Result<reqwest::Response> {
    let client_id = client_id()?;
    let mut token = load_token(token_key)?;

    if token.expiry <= now_unix() + 60 {
        token = do_refresh(&client_id, &token, token_key).await?;
    }

    let mut resp = http()?
        .get(url)
        .bearer_auth(token.access_token.expose())
        .send()
        .await?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        token = do_refresh(&client_id, &token, token_key).await?;
        resp = http()?
            .get(url)
            .bearer_auth(token.access_token.expose())
            .send()
            .await?;
    }
    if resp.status() == reqwest::StatusCode::TOO_MANY_REQUESTS {
        let wait = resp
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(2)
            .min(60);
        tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
        resp = http()?
            .get(url)
            .bearer_auth(token.access_token.expose())
            .send()
            .await?;
    }
    Ok(resp)
}

/// Read a response body into bytes, but never buffer more than `max` — a huge OneDrive file must not
/// be able to balloon memory.
async fn read_capped_bytes(resp: reqwest::Response, max: usize) -> Result<Vec<u8>> {
    use futures_util::StreamExt;
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if buf.len() + chunk.len() > max {
            return Err(Error::Other(
                "That OneDrive file is too large to index.".into(),
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
            "Microsoft Graph request failed ({status}): {detail}"
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
    code: &str,
    redirect_uri: &str,
    verifier: &str,
) -> Result<Token> {
    // Public client: NO client_secret.
    let params = [
        ("grant_type", "authorization_code"),
        ("code", code),
        ("redirect_uri", redirect_uri),
        ("client_id", client_id),
        ("code_verifier", verifier),
    ];
    let resp = http()?.post(TOKEN_ENDPOINT).form(&params).send().await?;
    token_from_response(resp).await
}

/// Refresh the access token under `token_key`; carries the existing refresh token forward when
/// Microsoft doesn't return a new one, and re-persists the blob. Public client — no secret.
async fn do_refresh(client_id: &str, current: &Token, token_key: &str) -> Result<Token> {
    let refresh = current
        .refresh_token
        .clone()
        .ok_or_else(|| Error::Other("Microsoft session expired — reconnect in Settings.".into()))?;
    let params = [
        ("grant_type", "refresh_token"),
        ("refresh_token", refresh.expose()),
        ("client_id", client_id),
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
            "Microsoft sign-in failed ({status}): {detail}"
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
    let raw = secrets::get_microsoft_token_for(token_key)?
        .ok_or_else(|| Error::Other("Not connected to Microsoft. Connect in Settings.".into()))?;
    serde_json::from_str(raw.expose())
        .map_err(|e| Error::Other(format!("stored Microsoft token unreadable: {e}")))
}

/// Persist a token blob under its account keychain key. Public so the connector can save the token
/// returned by [`run_consent`] once it knows which account it belongs to.
pub fn save_token(token_key: &str, token: &Token) -> Result<()> {
    let json = serde_json::to_string(token).map_err(|e| Error::Other(e.to_string()))?;
    secrets::set_microsoft_token_for(token_key, &json)
}

// --- PKCE + auth URL ---

/// A PKCE verifier/challenge pair. The verifier is hex (a valid PKCE charset); the challenge is the
/// base64url-nopad SHA-256 of the verifier (the S256 method).
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

/// Build Microsoft's consent URL. `offline_access` (carried in `scope`) yields the refresh token;
/// `prompt=select_account` lets the user pick which Microsoft account (so a second account can be
/// added). Pure, so it's unit-tested.
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
            ("response_mode", "query"),
            ("prompt", "select_account"),
        ],
    )
    .map_err(|e| Error::Other(format!("could not build auth URL: {e}")))?;
    Ok(url.to_string())
}

// --- loopback redirect server ---

/// Accept connections until the OAuth redirect arrives, validate the CSRF `state`, and return the
/// authorization `code`. Polls with a deadline so it can't hang forever (browsers also make stray
/// requests like /favicon.ico, which we ignore).
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
                "Timed out waiting for Microsoft sign-in. Please try again.".into(),
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
            let _ = write_page(&mut stream, "Waiting for Microsoft…");
            continue;
        };
        let params = parse_query(&target);

        if let Some(err) = params.get("error") {
            let _ = write_page(
                &mut stream,
                "Sign-in was cancelled. You can close this tab.",
            );
            return Err(Error::Other(format!(
                "Microsoft sign-in was declined: {err}"
            )));
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
                let _ = write_page(&mut stream, "Waiting for Microsoft…");
            }
        }
    }
}

/// The request target from the first request line: `GET /?code=… HTTP/1.1` → `/?code=…`.
fn request_target(request: &str) -> Option<String> {
    let line = request.lines().next()?;
    line.split_whitespace().nth(1).map(str::to_string)
}

/// Constant-time equality for the OAuth CSRF `state` token, so the comparison can't be turned into a
/// timing oracle. (The length check is fine to short-circuit — the token length is fixed, not secret.)
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
    fn auth_url_carries_pkce_offline_scope_and_no_secret() {
        let url = build_auth_url(
            "client-123",
            "http://127.0.0.1:54321",
            "chal",
            "state-abc",
            ONEDRIVE_SCOPE,
        )
        .unwrap();
        assert!(url.starts_with(AUTH_ENDPOINT));
        assert!(url.contains("code_challenge_method=S256"));
        assert!(url.contains("response_type=code"));
        assert!(url.contains("prompt=select_account"));
        // Read-only Files scope + offline_access (URL-encoded space is `+`).
        assert!(url.contains("Files.Read"));
        assert!(url.contains("offline_access"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A54321"));
        assert!(url.contains("state=state-abc"));
        // Public client — a secret must never appear in the authorize URL.
        assert!(!url.contains("client_secret"));
    }

    #[test]
    fn query_parsing_decodes_and_extracts_code() {
        let params = parse_query("/?state=abc&code=M.C107%2Ffoo");
        assert_eq!(params.get("code").unwrap(), "M.C107/foo");
        assert_eq!(params.get("state").unwrap(), "abc");
    }

    #[test]
    fn request_target_pulls_the_path() {
        let req = "GET /?code=xyz&state=s HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
        assert_eq!(request_target(req).unwrap(), "/?code=xyz&state=s");
    }
}
