// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What Google's answer to a write means. Pure: the status code and the error body in, a verdict out,
//! so every case is tested against the error shapes Google documents
//! (developers.google.com/workspace/calendar/api/guides/errors, read 2026-10-09).
//!
//! Retrying is decided here too, and kept to one retry. A rate limit is refused before Google applies
//! anything, so it's retried once for any request. A server error may have been applied, so it's
//! retried only where a second copy can't apply twice: a GET, or a PATCH or DELETE guarded by
//! `If-Match` (an applied first copy makes the etag stale). An insert isn't: Google warns it can't
//! guarantee to catch a repeated id, so after a server error the caller fetches the id PM chose before
//! sending anything again (plan rule R2). Anything else goes back to the user.

use serde_json::Value;

/// The request a verdict is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Insert,
    Patch,
    Delete,
}

/// What Google's answer means for PM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// 2xx.
    Ok,
    /// 412 `conditionNotMet`: the etag sent is no longer current.
    Conflict,
    /// 404 or 410: the event isn't there (deleted, or never visible to this account).
    Gone,
    /// 409 `duplicate`: an insert's id already exists (an earlier attempt landed).
    Duplicate,
    /// 403 `forbiddenForNonOrganizer`: only the organiser may change these fields.
    NotOrganizer,
    /// 403 without write access to the calendar, or a scope error while PM believes the scope is
    /// granted: either way the user can't fix it by consenting again.
    ReadOnly,
    /// 403 for a missing scope (`insufficientPermissions`, the shape Google's APIs share; not on the
    /// Calendar errors page, so INFERRED). The caller decides: ReadOnly when the token already holds
    /// the write scope, otherwise ask for consent.
    InsufficientScope,
    /// 403 `userRateLimitExceeded` / `rateLimitExceeded`, or 429. Worth one retry.
    RateLimited,
    /// 403 `quotaExceeded`: the account's daily Calendar usage limit. Not worth retrying today.
    QuotaExceeded,
    /// 401: the access token was refused even after the refresh the sender already tried.
    Reauth,
    /// 400: a request Google won't take, with its message.
    BadRequest(String),
    /// 5xx. Worth one retry.
    ServerError,
    /// Any other status, with Google's message.
    Other(u16, String),
}

/// Read Google's verdict on a write from its status and body.
pub fn classify(status: u16, body: &str) -> Verdict {
    if (200..300).contains(&status) {
        return Verdict::Ok;
    }
    let (reason, message) = error_reason(body);
    let reason = reason.as_deref().unwrap_or("");
    match status {
        400 => Verdict::BadRequest(message),
        401 => Verdict::Reauth,
        403 => match reason {
            "forbiddenForNonOrganizer" => Verdict::NotOrganizer,
            "userRateLimitExceeded" | "rateLimitExceeded" => Verdict::RateLimited,
            "quotaExceeded" => Verdict::QuotaExceeded,
            "insufficientPermissions" | "ACCESS_TOKEN_SCOPE_INSUFFICIENT" => {
                Verdict::InsufficientScope
            }
            // `requiredAccessLevel`, `forbidden`, or anything else: no write access here.
            _ => Verdict::ReadOnly,
        },
        404 | 410 => Verdict::Gone,
        409 if reason == "duplicate" => Verdict::Duplicate,
        412 => Verdict::Conflict,
        429 => Verdict::RateLimited,
        500..=599 => Verdict::ServerError,
        other => Verdict::Other(other, message),
    }
}

/// Whether to send `method` once more after `verdict`. Only ever once (`attempt` counts from 1).
pub fn should_retry(method: Method, verdict: &Verdict, attempt: u32) -> bool {
    if attempt > 1 {
        return false;
    }
    match verdict {
        // Refused before anything was applied.
        Verdict::RateLimited => true,
        // Maybe applied. Spelled out per method so a new one has to decide.
        Verdict::ServerError => match method {
            Method::Get | Method::Patch | Method::Delete => true,
            Method::Insert => false,
        },
        _ => false,
    }
}

/// The first `reason` and the message from a Google error body, the message cut to one plain line of
/// at most 300 characters (it reaches the UI as text). Tolerates a body that isn't JSON.
fn error_reason(body: &str) -> (Option<String>, String) {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let error = parsed.as_ref().and_then(|v| v.get("error"));
    let reason = error
        .and_then(|e| e.get("errors"))
        .and_then(Value::as_array)
        .and_then(|errs| errs.first())
        .and_then(|e| e.get("reason"))
        .and_then(Value::as_str)
        .or_else(|| {
            // The newer shape names it under `details[].reason` (e.g. a scope error).
            error
                .and_then(|e| e.get("details"))
                .and_then(Value::as_array)
                .and_then(|d| {
                    d.iter()
                        .find_map(|x| x.get("reason").and_then(Value::as_str))
                })
        })
        .map(str::to_string);
    let message = error
        .and_then(|e| e.get("message"))
        .and_then(Value::as_str)
        .unwrap_or(body);
    let message: String = message
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(300)
        .collect();
    (reason, message.trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An error body in the shape Google's Calendar errors guide shows.
    fn google_error(code: u16, reason: &str, message: &str) -> String {
        serde_json::json!({
            "error": {
                "errors": [{ "domain": "global", "reason": reason, "message": message }],
                "code": code,
                "message": message
            }
        })
        .to_string()
    }

    /// Every error the Calendar errors guide documents, with its own reason and message.
    #[test]
    fn every_documented_error_has_a_verdict() {
        let cases = [
            (
                400,
                "timeRangeEmpty",
                "The specified time range is empty.",
                Verdict::BadRequest("The specified time range is empty.".into()),
            ),
            (401, "authError", "Invalid Credentials", Verdict::Reauth),
            (
                403,
                "userRateLimitExceeded",
                "User Rate Limit Exceeded",
                Verdict::RateLimited,
            ),
            (
                403,
                "rateLimitExceeded",
                "Rate Limit Exceeded",
                Verdict::RateLimited,
            ),
            (
                403,
                "quotaExceeded",
                "Calendar usage limits exceeded.",
                Verdict::QuotaExceeded,
            ),
            (
                403,
                "forbiddenForNonOrganizer",
                "Shared properties can only be changed by the organizer of the event.",
                Verdict::NotOrganizer,
            ),
            (404, "notFound", "Not Found", Verdict::Gone),
            (
                409,
                "duplicate",
                "The requested identifier already exists.",
                Verdict::Duplicate,
            ),
            (410, "deleted", "Resource has been deleted", Verdict::Gone),
            (
                412,
                "conditionNotMet",
                "Precondition Failed",
                Verdict::Conflict,
            ),
            (
                429,
                "rateLimitExceeded",
                "Rate Limit Exceeded",
                Verdict::RateLimited,
            ),
            (500, "backendError", "Backend Error", Verdict::ServerError),
        ];
        for (status, reason, message, want) in cases {
            assert_eq!(
                classify(status, &google_error(status, reason, message)),
                want,
                "{status} {reason}"
            );
        }
    }

    #[test]
    fn write_access_and_scope_errors_are_told_apart() {
        // Not on the Calendar guide (INFERRED from Google's shared error shapes): no write access, and
        // a token without the scope.
        assert_eq!(
            classify(
                403,
                &google_error(
                    403,
                    "requiredAccessLevel",
                    "You need to have writer access to this calendar."
                )
            ),
            Verdict::ReadOnly
        );
        assert_eq!(
            classify(
                403,
                &google_error(
                    403,
                    "insufficientPermissions",
                    "Request had insufficient authentication scopes."
                )
            ),
            Verdict::InsufficientScope
        );
        let newer = serde_json::json!({ "error": {
            "code": 403, "message": "Request had insufficient authentication scopes.",
            "status": "PERMISSION_DENIED",
            "details": [{ "@type": "type.googleapis.com/google.rpc.ErrorInfo",
                          "reason": "ACCESS_TOKEN_SCOPE_INSUFFICIENT" }]
        }})
        .to_string();
        assert_eq!(classify(403, &newer), Verdict::InsufficientScope);
        // A 403 PM can't read at all is read-only, never "consent again" (that would loop).
        assert_eq!(classify(403, "<html>Forbidden</html>"), Verdict::ReadOnly);
    }

    #[test]
    fn a_batch_conflict_and_odd_statuses_fall_through_with_their_message() {
        assert_eq!(
            classify(409, &google_error(409, "conflict", "Conflict")),
            Verdict::Other(409, "Conflict".into())
        );
        assert_eq!(classify(204, ""), Verdict::Ok);
        assert_eq!(classify(503, "Service Unavailable"), Verdict::ServerError);
        // Google's message reaches the UI as one clipped plain line.
        let long = format!("bad\nrequest {}", "x".repeat(400));
        match classify(400, &google_error(400, "invalid", &long)) {
            Verdict::BadRequest(m) => {
                assert!(!m.contains('\n'));
                assert_eq!(m.chars().count(), 300);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn only_transient_failures_are_retried_and_only_once() {
        for method in [Method::Get, Method::Insert, Method::Patch, Method::Delete] {
            assert!(should_retry(method, &Verdict::RateLimited, 1));
            // A server error may have applied an insert: the caller fetches the id first instead.
            assert_eq!(
                should_retry(method, &Verdict::ServerError, 1),
                method != Method::Insert,
                "{method:?}"
            );
            assert!(!should_retry(method, &Verdict::ServerError, 2));
            assert!(!should_retry(method, &Verdict::RateLimited, 2));
            for settled in [
                Verdict::Conflict,
                Verdict::Gone,
                Verdict::ReadOnly,
                Verdict::QuotaExceeded,
                Verdict::Reauth,
                Verdict::Duplicate,
            ] {
                assert!(!should_retry(method, &settled, 1), "{method:?} {settled:?}");
            }
        }
    }
}
