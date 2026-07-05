// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Shared OAuth loopback machinery for the [`crate::google`] and [`crate::microsoft`] connectors.
//!
//! Both providers use the same RFC 8252 native-app flow — PKCE + a one-shot loopback HTTP server
//! that catches the browser redirect on `http://127.0.0.1:<ephemeral-port>` — and the code for it was
//! duplicated verbatim across the two modules (down to the constant-time state check and the tiny
//! success page). This module owns that flow once; each provider keeps only what genuinely differs
//! (its `build_auth_url` params, token shapes, and refresh policy).
//!
//! It also owns the **per-key refresh lock** ([`refresh_lock`]): the coordination primitive both
//! providers were missing. Two syncs sharing one account's keychain key (e.g. a Drive sync and a
//! Drive backup, or the proactive and reactive refresh paths racing) could each notice the token was
//! near expiry and refresh concurrently. For a provider that ROTATES the refresh token on use
//! (Microsoft does), the second refresh would post an already-invalidated refresh token and wedge the
//! account. Serializing refreshes per key — and reloading the token blob under the lock — closes that
//! race for both providers.

use base64::Engine;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

use crate::error::{Error, Result};

/// How long to wait for the browser consent redirect before giving up.
const REDIRECT_TIMEOUT_SECS: u64 = 180;

/// Unix seconds now, for token-expiry math. Saturates to 0 if the clock is before the epoch (it isn't).
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// --- PKCE ---

/// A PKCE verifier/challenge pair. The verifier is hex (a valid PKCE charset); the challenge is the
/// base64url-nopad SHA-256 of the verifier (the S256 method).
pub fn pkce() -> Result<(String, String)> {
    let verifier = random_token(32)?;
    let digest = Sha256::digest(verifier.as_bytes());
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
    Ok((verifier, challenge))
}

/// `n` random bytes, hex-encoded — used for the PKCE verifier and the CSRF state.
pub fn random_token(n: usize) -> Result<String> {
    let mut bytes = vec![0u8; n];
    getrandom::fill(&mut bytes).map_err(|e| Error::Other(format!("rng failure: {e}")))?;
    Ok(hex::encode(bytes))
}

// --- loopback redirect server ---

/// Accept connections until the OAuth redirect arrives, validate the CSRF `state`, and return the
/// authorization `code`. `provider` names the service in the "waiting"/"declined" strings (e.g.
/// "Google"); `success_label` names the connected product on the success page. Polls with a deadline
/// so it can't hang forever (browsers also make stray requests like /favicon.ico, which we ignore).
pub fn wait_for_redirect(
    listener: std::net::TcpListener,
    expected_state: &str,
    provider: &str,
    success_label: &str,
) -> Result<String> {
    use std::io::Read;

    listener.set_nonblocking(true)?;
    let deadline =
        std::time::Instant::now() + std::time::Duration::from_secs(REDIRECT_TIMEOUT_SECS);

    loop {
        if std::time::Instant::now() >= deadline {
            return Err(Error::Other(format!(
                "Timed out waiting for {provider} sign-in. Please try again."
            )));
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
            let _ = write_page(&mut stream, &format!("Waiting for {provider}…"));
            continue;
        };
        let params = parse_query(&target);

        if let Some(err) = params.get("error") {
            let _ = write_page(
                &mut stream,
                "Sign-in was cancelled. You can close this tab.",
            );
            return Err(Error::Other(format!(
                "{provider} sign-in was declined: {err}"
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
                let _ = write_page(&mut stream, &format!("Waiting for {provider}…"));
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
fn parse_query(target: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
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

// --- per-key refresh lock ---

/// One async mutex per keychain token key, minted on first use. `std::sync::Mutex` guards only the map
/// insert (never held across an `.await`); the per-key `tokio` mutex is what's held across the refresh
/// network round-trip. The map is tiny (one entry per connected account) and never needs eviction.
static REFRESH_LOCKS: LazyLock<Mutex<HashMap<String, Arc<AsyncMutex<()>>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Acquire the refresh lock for `token_key`, serializing token refreshes that share it. The caller
/// holds the returned guard for the duration of its refresh (reload-under-lock → refresh → persist),
/// so a concurrent refresh of the same key waits rather than racing — the fix for a rotated refresh
/// token being posted twice (which would wedge a Microsoft account). Different keys don't contend.
pub async fn refresh_lock(token_key: &str) -> OwnedMutexGuard<()> {
    let lock = {
        let mut map = REFRESH_LOCKS
            .lock()
            .expect("refresh-lock registry mutex poisoned");
        map.entry(token_key.to_string()).or_default().clone()
    };
    lock.lock_owned().await
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
    fn pkce_pair_is_random_and_well_formed() {
        let (v1, c1) = pkce().unwrap();
        let (v2, _) = pkce().unwrap();
        assert_ne!(v1, v2, "each verifier is freshly random");
        assert_eq!(v1.len(), 64, "32 random bytes, hex-encoded");
        // The challenge is the base64url-nopad S256 of the verifier we just made.
        let digest = Sha256::digest(v1.as_bytes());
        let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
        assert_eq!(c1, expected);
    }

    #[test]
    fn query_parsing_decodes_and_extracts_code() {
        // `%2F` → `/`, `%2C` → `,`, `+` → space — the Google/Microsoft redirect shapes both files tested.
        let params = parse_query("/?state=abc&code=4%2F0Ab%2Cd&note=a+b");
        assert_eq!(params.get("code").unwrap(), "4/0Ab,d");
        assert_eq!(params.get("state").unwrap(), "abc");
        assert_eq!(params.get("note").unwrap(), "a b");
    }

    #[test]
    fn request_target_pulls_the_path() {
        let req = "GET /?code=xyz&state=s HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
        assert_eq!(request_target(req).unwrap(), "/?code=xyz&state=s");
    }

    #[test]
    fn ct_eq_matches_only_equal_strings() {
        assert!(ct_eq("state-abc", "state-abc"));
        assert!(!ct_eq("state-abc", "state-abd"));
        assert!(!ct_eq("state-abc", "state-abcd")); // differing length
    }
}
