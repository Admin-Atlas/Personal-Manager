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

/// True once an OAuth token is stored (the user has completed sign-in).
pub fn is_connected() -> Result<bool> {
    Ok(secrets::get_google_token()?.is_some())
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

fn http() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(Error::from)
}

/// Run the full connect flow: open the browser to Google's consent screen, catch the
/// loopback redirect, exchange the code for tokens, and store them. Errors (no client
/// configured, browser failed to open, user cancelled, timeout) surface to the UI.
pub async fn connect() -> Result<()> {
    let (client_id, client_secret) = client_creds()?;
    let (verifier, challenge) = pkce()?;
    let state = random_token(16)?;

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
        CALENDAR_SCOPE,
    )?;
    open::that(&auth_url)
        .map_err(|e| Error::Other(format!("Couldn't open your browser to sign in: {e}")))?;

    // Wait for Google to redirect back with the code (blocking accept, off-runtime).
    let expected_state = state.clone();
    let code = tokio::task::spawn_blocking(move || wait_for_redirect(listener, &expected_state))
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
    save_token(&token)
}

/// GET a Google API URL with a valid bearer token, JSON-decoded. Refreshes the
/// access token first if it's near expiry, and retries once after a refresh on 401.
/// Never touches the DB, so callers hold no lock across it (rule #4).
pub async fn authorized_get(url: &str) -> Result<serde_json::Value> {
    let (client_id, client_secret) = client_creds()?;
    let mut token = load_token()?;

    // Refresh proactively a minute before expiry so a request doesn't fail mid-flight;
    // the 401 retry below is the backstop for a token that's revoked or expires early.
    if token.expiry <= now_unix() + 60 {
        token = do_refresh(&client_id, client_secret.expose(), &token).await?;
    }

    let resp = http()?
        .get(url)
        .bearer_auth(token.access_token.expose())
        .send()
        .await?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        let token = do_refresh(&client_id, client_secret.expose(), &token).await?;
        let resp = http()?
            .get(url)
            .bearer_auth(token.access_token.expose())
            .send()
            .await?;
        return json_or_err(resp).await;
    }
    json_or_err(resp).await
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

/// Refresh the access token; carries the existing refresh token forward when Google
/// doesn't return a new one (it usually doesn't), and re-persists the blob.
async fn do_refresh(client_id: &str, client_secret: &str, current: &Token) -> Result<Token> {
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
    save_token(&token)?;
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

fn load_token() -> Result<Token> {
    let raw = secrets::get_google_token()?
        .ok_or_else(|| Error::Other("Not connected to Google. Connect in Settings.".into()))?;
    serde_json::from_str(raw.expose())
        .map_err(|e| Error::Other(format!("stored Google token unreadable: {e}")))
}

fn save_token(token: &Token) -> Result<()> {
    let json = serde_json::to_string(token).map_err(|e| Error::Other(e.to_string()))?;
    secrets::set_google_token(&json)
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
/// refresh token so PM can stay connected. Pure, so it's unit-tested.
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
            ("prompt", "consent"),
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
fn wait_for_redirect(listener: std::net::TcpListener, expected_state: &str) -> Result<String> {
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
                    "PM is connected to Google Calendar. You can close this tab and return to PM.",
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
        assert!(url.contains("prompt=consent"));
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
