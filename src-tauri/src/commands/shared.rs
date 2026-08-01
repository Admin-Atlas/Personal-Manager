// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The two helpers with no owning surface, and the shared test fixture.
//!
//! Deliberately small. A helper belongs with the module that owns its subject; only one
//! that would otherwise have to pick arbitrarily between two callers lands here.

use rusqlite::Connection;

use crate::db;
use crate::error::{Error, Result};
use crate::settings::TIME_ZONE_KEY;

/// Normalize the optional per-account client (id + secret) passed at connect time into
/// `Some((id, secret))` only when BOTH are non-empty; blank means "use the shared client". Lets an
/// Advanced-Protection account sign in with its own Cloud project (see
/// [`secrets::set_google_client_for_account`]). Errors if exactly one of the two is supplied.
pub(super) fn own_client(
    client_id: Option<String>,
    client_secret: Option<String>,
) -> Result<Option<(String, String)>> {
    let id = client_id.unwrap_or_default().trim().to_string();
    let secret = client_secret.unwrap_or_default().trim().to_string();
    match (id.is_empty(), secret.is_empty()) {
        (true, true) => Ok(None),
        (false, false) => Ok(Some((id, secret))),
        _ => Err(Error::Other(
            "Enter both the account's Client ID and Client secret, or leave both blank to use the \
             shared client."
                .into(),
        )),
    }
}

// --- helpers ---
// NOTE: there is deliberately no `iso_now(&AppState)` helper here. One existed and took
// `state.conn()` internally, which self-deadlocked the non-reentrant DB mutex when called
// with the guard already held (it froze every fresh-vault boot). Use `ingest::iso_now(&conn)`
// with the connection you already hold.

/// Resolve the user's stored IANA zone to a `chrono_tz::Tz`. Falls back to UTC when
/// the key is unset, empty, or unparseable — chrono `Local` only yields an offset
/// (no IANA name, DST-unstable), so the canonical zone is supplied by the frontend
/// (`Intl`) and stored; UTC is the stable default matching every `strftime('now')`.
/// Infallible by design (worst case UTC) so call sites stay one-liners.
pub(crate) fn resolve_zone(conn: &Connection) -> chrono_tz::Tz {
    use std::str::FromStr;
    db::get_setting(conn, TIME_ZONE_KEY)
        .ok()
        .flatten()
        .and_then(|s| chrono_tz::Tz::from_str(s.trim()).ok())
        .unwrap_or(chrono_tz::Tz::UTC)
}

/// A throwaway encrypted store (also exercises the migration-in-transaction
/// path in `db::open`).
#[cfg(test)]
pub(super) fn temp_db() -> (tempfile::TempDir, Connection) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("t.sqlite");
    let key = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";
    let conn = db::open(&path, key).unwrap();
    (dir, conn)
}
