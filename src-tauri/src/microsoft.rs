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

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::oauth_loopback;
use crate::secret::Secret;
use crate::secrets;

/// `/common` so both personal Microsoft accounts (consumers) and work/school accounts (orgs) can
/// sign in; the user's app registration must allow both account types for this to resolve.
const AUTH_ENDPOINT: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/authorize";
const TOKEN_ENDPOINT: &str = "https://login.microsoftonline.com/common/oauth2/v2.0/token";
/// Microsoft Graph base for the OneDrive connector's API calls (drive items, delta, content, /me).
pub const GRAPH_API: &str = "https://graph.microsoft.com/v1.0";
/// The Graph service roots PM may hand an account bearer to. PM authorises against the GLOBAL
/// `/common` authority (see [`AUTH_ENDPOINT`]) and builds every URL from [`GRAPH_API`], and a
/// national-cloud token is not interchangeable with a global one, so `graph.microsoft.com` is the
/// only host that can legitimately serve one of PM's continuation links. The other documented
/// roots — `graph.microsoft.us` (US Gov L4/GCC High), `dod-graph.microsoft.us` (US Gov L5/DoD),
/// `microsoftgraph.chinacloudapi.cn` (China 21Vianet) — belong here only if `GRAPH_API` ever
/// becomes configurable. Source: Microsoft Learn, "Microsoft Graph national cloud deployments".
const GRAPH_HOSTS: &[&str] = &["graph.microsoft.com"];
/// Read-only OneDrive scope + `offline_access` (for the refresh token) + `User.Read` (to learn which
/// account the token grants, the Graph equivalent of Drive's `about`). Space-separated, as Graph
/// expects. `Files.Read` covers reading file metadata AND content (index-only needs each body to
/// embed it).
pub const ONEDRIVE_SCOPE: &str = "Files.Read offline_access User.Read";
/// Read-only Outlook/Microsoft 365 calendar scope (card 6A) + `offline_access` (refresh token) +
/// `User.Read` (to learn which account the token grants, via `/me`). `Calendars.Read` reads events
/// from every calendar the account owns or is shared on; PM never writes (spec non-goal #4).
pub const CALENDAR_SCOPE: &str = "Calendars.Read offline_access User.Read";

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
    let (verifier, challenge) = oauth_loopback::pkce()?;
    let state = oauth_loopback::random_token(16)?;

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
    let code = tokio::task::spawn_blocking(move || {
        oauth_loopback::wait_for_redirect(listener, &expected_state, "Microsoft", &label)
    })
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

/// Whether `url` is one PM may attach an account bearer to: https, no userinfo, and an EXACT
/// (case-insensitive) match on an allow-listed Graph service root ([`GRAPH_HOSTS`]). Exact host
/// equality, never a `.microsoft.com` suffix test — a suffix rule admits every Microsoft-hosted
/// subdomain, including user-content ones like `*.sharepoint.com`'s siblings.
///
/// This exists because six call sites (OneDrive delta/children/picker, Outlook calendar list and
/// `calendarView`) follow an absolute `@odata.nextLink` / `@odata.deltaLink` that came out of a
/// provider response BODY, i.e. data, not configuration.
pub(crate) fn is_graph_url(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|u| {
        u.scheme() == "https"
            && u.username().is_empty()
            && u.password().is_none()
            && u.host_str()
                .is_some_and(|h| GRAPH_HOSTS.iter().any(|g| h.eq_ignore_ascii_case(g)))
    })
}

/// Shared authorised GET: proactive refresh a minute before expiry, a single 401-retry-after-refresh
/// (the backstop for a token revoked or expired early), and a single bounded `Retry-After` retry on a
/// 429 throttle (Graph throttles initial enumerations harder than Drive). Returns the raw response so
/// the caller can decode it as JSON or bytes.
///
/// The bearer is attached only for an allow-listed Graph host. An unexpected host is followed
/// WITHOUT it rather than refused outright, so a surprise Microsoft CDN degrades sync instead of
/// breaking it; the account then flags `error` on the inevitable 401 rather than silently shipping
/// a `Files.Read`-scoped token to whoever the response body named. Note the gate applies to the
/// INITIAL URL only — reqwest already strips `Authorization` across a cross-host redirect, which is
/// exactly what keeps the `/content` 302 to `*.sharepoint.com` / `*.files.1drv.com` both working
/// and safe, so redirect policy must not be touched here.
async fn authorized_send(token_key: &str, url: &str) -> Result<reqwest::Response> {
    if !is_graph_url(url) {
        // Skips `load_token`/`do_refresh` entirely: a poisoned link costs no keychain read and
        // can never trigger a refresh. Still DB-free, so rule #4 is unaffected.
        eprintln!("microsoft: following a non-Graph link unauthenticated");
        return Ok(http()?.get(url).send().await?);
    }
    let client_id = client_id()?;
    let mut token = load_token(token_key)?;

    // Lock-free fast path; `do_refresh` re-checks expiry under the per-key lock before any network call.
    if token.expiry <= oauth_loopback::now_unix() + 60 {
        token = do_refresh(&client_id, token_key, false).await?;
    }

    let mut resp = http()?
        .get(url)
        .bearer_auth(token.access_token.expose())
        .send()
        .await?;
    if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
        token = do_refresh(&client_id, token_key, true).await?;
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
///
/// Serialized per key by the shared [`oauth_loopback::refresh_lock`], reloading the blob *under* the
/// lock. This matters more for Microsoft than Google: Entra ROTATES the refresh token on every use, so
/// two concurrent refreshes racing would have the loser post an already-invalidated refresh token and
/// wedge the account. Reloading under the lock means the loser refreshes from the winner's freshly
/// persisted (rotated) token instead. `force = false` (proactive) returns early when the reloaded token
/// is already fresh; `force = true` (reactive 401) always refreshes (the token may be revoked).
async fn do_refresh(client_id: &str, token_key: &str, force: bool) -> Result<Token> {
    let _guard = oauth_loopback::refresh_lock(token_key).await;
    let current = load_token(token_key)?;
    if !force && current.expiry > oauth_loopback::now_unix() + 60 {
        return Ok(current);
    }
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
        expiry: oauth_loopback::now_unix() + t.expires_in.unwrap_or(3600),
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

// --- auth URL (PKCE + loopback machinery live in `crate::oauth_loopback`) ---

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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn is_graph_url_accepts_only_the_allow_listed_service_root() {
        // The shapes PM actually follows: a delta/continuation link off the Graph root, and the
        // same host in any casing (a hostile body can vary case freely).
        assert!(is_graph_url(
            "https://graph.microsoft.com/v1.0/me/drive/root/delta?token=X"
        ));
        assert!(is_graph_url("https://GRAPH.MICROSOFT.COM/v1.0/me"));
    }

    #[test]
    fn is_graph_url_rejects_the_near_miss_set() {
        for bad in [
            // A suffix test would admit this one — hence exact host equality.
            "https://graph.microsoft.com.evil.example/v1.0/me",
            // The allow-listed host present only as data inside someone else's URL.
            "https://evil.example/v1.0/me/drive?u=https://graph.microsoft.com",
            "http://graph.microsoft.com/v1.0/me", // cleartext
            "https://user:pw@graph.microsoft.com/v1.0/me", // userinfo
            "https://graph.microsoft.us/v1.0/me", // a real Graph root PM is not configured for
            "not a url at all",
            "",
        ] {
            assert!(!is_graph_url(bad), "should have been rejected: {bad}");
        }
    }

    /// Drift pin: changing `GRAPH_API` without updating `GRAPH_HOSTS` would silently
    /// un-authenticate every OneDrive and Outlook-calendar call rather than fail loudly.
    #[test]
    fn the_graph_base_url_is_itself_allow_listed() {
        assert!(is_graph_url(GRAPH_API));
    }
}
