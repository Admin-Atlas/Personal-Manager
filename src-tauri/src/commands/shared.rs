// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The helpers with no owning surface, and the shared test fixture.
//!
//! Deliberately small. A helper belongs with the module that owns its subject; only one
//! that would otherwise have to pick arbitrarily between two callers lands here.

use std::collections::BTreeSet;

use rusqlite::{params, Connection};
use serde::Serialize;

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

// --- one Google grant, several PM features ---
//
// Calendar, Drive (with Sheets) and the Drive backup all sign in to a Google account through the
// same OAuth client, so they share one grant at Google — and revoking ANY of their tokens removes
// that whole grant ("Revocation removes all OAuth 2.0 scopes previously granted to a project", per
// Google's native-app OAuth guide). They also share the account's own client when it has one (an
// Advanced-Protection account; `google::client_creds_for_key` resolves it from the email suffix of
// every Google token key). So a disconnect may revoke, or forget that client, only when it is the
// last PM feature still using the account. Both Google disconnects decide that here, in one place.

/// One PM feature that signs in to a Google account. Serialized into [`GoogleDisconnect`], whose
/// strings the UI's `GoogleUse` union mirrors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum GoogleUse {
    Calendar,
    Drive,
    Backup,
}

/// What a Google disconnect returns: the features that still use the account, and so kept PM's grant
/// at Google alive. Empty when the disconnect was the last one and the grant was revoked.
#[derive(Debug, Serialize)]
pub struct GoogleDisconnect {
    pub kept_for: Vec<GoogleUse>,
}

/// Every PM feature currently signed in to `email`'s Google account. Emails match without regard to
/// case: Calendar stores them lowercased, Drive as Google's `about` returns them, and the backup
/// setting as the user's choice. An empty backup setting means "no backup account" (disconnecting
/// backup writes `""` rather than deleting the key).
pub(super) fn google_grant_users(conn: &Connection, email: &str) -> Result<BTreeSet<GoogleUse>> {
    let mut stmt = conn.prepare(
        "SELECT service FROM connector_sources \
         WHERE provider = 'google' AND account_email IS NOT NULL \
           AND lower(account_email) = lower(?1)",
    )?;
    let services: Vec<String> = stmt
        .query_map(params![email], |r| r.get(0))?
        .collect::<std::result::Result<_, _>>()?;
    let mut users: BTreeSet<GoogleUse> = services
        .iter()
        .filter_map(|s| match s.as_str() {
            crate::calendar::SERVICE => Some(GoogleUse::Calendar),
            crate::drive::SERVICE => Some(GoogleUse::Drive),
            _ => None,
        })
        .collect();
    let backup = db::get_setting(conn, crate::backup::schedule::BACKUP_GDRIVE_ACCOUNT_KEY)?;
    if backup.is_some_and(|a| !a.is_empty() && a.eq_ignore_ascii_case(email)) {
        users.insert(GoogleUse::Backup);
    }
    Ok(users)
}

/// What one Google disconnect may release, given every feature using the account
/// ([`google_grant_users`]) and the one leaving.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct GrantRelease {
    /// Revoke at Google — only when nothing else uses the account, since a revoke ends the grant for
    /// every feature at once.
    pub revoke: bool,
    /// Forget the account's own (Advanced-Protection) client — same condition: the remaining
    /// features' tokens refresh through it.
    pub forget_account_client: bool,
    /// Keep the leaving feature's own token. Only Drive leaving while backup stays: the backup signs
    /// in through the Drive token key, and a re-granted `drive.file` has no authority over the
    /// archives the old grant uploaded (#600).
    pub keep_own_token: bool,
    /// The features that keep the grant alive, for the UI to name.
    pub kept_for: Vec<GoogleUse>,
}

/// Decide what a disconnect of `leaving` may release. Pure, so every combination is table-tested.
pub(super) fn release_plan(users: &BTreeSet<GoogleUse>, leaving: GoogleUse) -> GrantRelease {
    let kept_for: Vec<GoogleUse> = users.iter().copied().filter(|u| *u != leaving).collect();
    let last = kept_for.is_empty();
    GrantRelease {
        revoke: last,
        forget_account_client: last,
        keep_own_token: leaving == GoogleUse::Drive && kept_for.contains(&GoogleUse::Backup),
        kept_for,
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

#[cfg(test)]
mod tests {
    use super::*;
    use GoogleUse::{Backup, Calendar, Drive};

    fn set(uses: &[GoogleUse]) -> BTreeSet<GoogleUse> {
        uses.iter().copied().collect()
    }

    /// Every combination of features on an account, for each feature that can disconnect. A disconnect
    /// releases the grant (revoke + the account's own client) only as the account's last user, and the
    /// Drive token outlives a Drive disconnect only while backup signs in through it (#600).
    #[test]
    fn release_plan_releases_the_grant_only_for_the_last_user() {
        let all = [Calendar, Drive, Backup];
        for mask in 0..8u8 {
            let users: BTreeSet<GoogleUse> = (0..3)
                .filter(|i| mask & (1 << i) != 0)
                .map(|i| all[i])
                .collect();
            for leaving in [Calendar, Drive] {
                let plan = release_plan(&users, leaving);
                let others: Vec<GoogleUse> =
                    users.iter().copied().filter(|u| *u != leaving).collect();
                assert_eq!(plan.kept_for, others, "{users:?} leaving {leaving:?}");
                assert_eq!(
                    plan.revoke,
                    others.is_empty(),
                    "{users:?} leaving {leaving:?}"
                );
                assert_eq!(plan.forget_account_client, others.is_empty());
                assert_eq!(
                    plan.keep_own_token,
                    leaving == Drive && others.contains(&Backup),
                    "{users:?} leaving {leaving:?}"
                );
            }
        }
    }

    #[test]
    fn release_plan_pins_the_cases_that_used_to_go_wrong() {
        // Calendar leaving while Drive stays: today's bug revoked here and cut Drive off.
        assert_eq!(
            release_plan(&set(&[Calendar, Drive]), Calendar),
            GrantRelease {
                revoke: false,
                forget_account_client: false,
                keep_own_token: false,
                kept_for: vec![Drive],
            }
        );
        // Drive leaving while Calendar stays: the new direction. Drive's token goes, the client stays.
        assert_eq!(
            release_plan(&set(&[Calendar, Drive]), Drive),
            GrantRelease {
                revoke: false,
                forget_account_client: false,
                keep_own_token: false,
                kept_for: vec![Calendar],
            }
        );
        // Drive leaving while backup stays: the #600 guard, which nothing pinned before.
        assert_eq!(
            release_plan(&set(&[Drive, Backup]), Drive),
            GrantRelease {
                revoke: false,
                forget_account_client: false,
                keep_own_token: true,
                kept_for: vec![Backup],
            }
        );
        // The last user releases everything.
        assert_eq!(
            release_plan(&set(&[Calendar]), Calendar),
            GrantRelease {
                revoke: true,
                forget_account_client: true,
                keep_own_token: false,
                kept_for: vec![],
            }
        );
    }

    #[test]
    fn google_grant_users_matches_the_account_whatever_its_case() {
        let (_dir, conn) = temp_db();
        // Calendar stores the email lowercased, Drive as Google returns it, backup as chosen.
        crate::calendar::upsert_source(
            &conn,
            &crate::calendar::google_account_id("me@example.com"),
            "google",
            Some("me@example.com"),
            "me@example.com",
        )
        .unwrap();
        crate::drive::upsert_account(&conn, "Me@Example.com", "Me").unwrap();
        db::set_setting(
            &conn,
            crate::backup::schedule::BACKUP_GDRIVE_ACCOUNT_KEY,
            "ME@EXAMPLE.COM",
        )
        .unwrap();
        // Not this Google account: an Outlook calendar on the same address, an iCal feed tagged
        // "google" (no account email), and another Google account.
        crate::calendar::upsert_source(
            &conn,
            "outlook:me@example.com",
            "microsoft",
            Some("me@example.com"),
            "Me",
        )
        .unwrap();
        crate::calendar::upsert_source(&conn, "ics:abc", "google", None, "Holidays").unwrap();
        crate::drive::upsert_account(&conn, "other@example.com", "Other").unwrap();

        assert_eq!(
            google_grant_users(&conn, "me@example.com").unwrap(),
            set(&[Calendar, Drive, Backup])
        );
        assert_eq!(
            google_grant_users(&conn, "other@example.com").unwrap(),
            set(&[Drive])
        );
        assert_eq!(
            google_grant_users(&conn, "nobody@example.com").unwrap(),
            set(&[])
        );
    }

    #[test]
    fn an_emptied_backup_setting_is_no_backup_account() {
        let (_dir, conn) = temp_db();
        // Disconnecting backup writes "" rather than deleting the key.
        db::set_setting(
            &conn,
            crate::backup::schedule::BACKUP_GDRIVE_ACCOUNT_KEY,
            "",
        )
        .unwrap();
        assert_eq!(google_grant_users(&conn, "").unwrap(), set(&[]));
    }

    /// The UI's `GoogleUse` union and the grant note match these exact strings.
    #[test]
    fn google_use_serializes_to_the_strings_the_ui_names() {
        let json = serde_json::to_string(&GoogleDisconnect {
            kept_for: vec![Calendar, Drive, Backup],
        })
        .unwrap();
        assert_eq!(json, r#"{"kept_for":["calendar","drive","backup"]}"#);
    }
}
