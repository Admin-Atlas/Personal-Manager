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
// Advanced-Protection account; every token that client minted refreshes through it, see
// `google::client_creds_for_token`). So a disconnect may revoke, or forget that client, only when it
// is the last PM feature still using the account. Both Google disconnects decide that here, in one
// place.

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
    /// The kept grant still lets PM change the account's calendar events: editing was granted once and
    /// never revoked ([`crate::calendar_editing::write_granted`]). Only a Calendar disconnect sets it.
    pub calendar_write: bool,
}

/// Every PM feature currently signed in to `email`'s Google account. Emails match without regard to
/// case: Calendar stores them lowercased, Drive as Google's `about` returns them, and the backup
/// setting as the user's choice. An empty backup setting means "no backup account" (disconnecting
/// backup writes `""` rather than deleting the key).
pub(super) fn google_grant_users(conn: &Connection, email: &str) -> Result<BTreeSet<GoogleUse>> {
    let mut users: BTreeSet<GoogleUse> = google_connector_rows(conn, email)?
        .iter()
        .filter_map(|(s, _)| match s.as_str() {
            crate::calendar::SERVICE => Some(GoogleUse::Calendar),
            crate::drive::SERVICE => Some(GoogleUse::Drive),
            _ => None,
        })
        .collect();
    if backup_account(conn)?.is_some_and(|a| a.eq_ignore_ascii_case(email)) {
        users.insert(GoogleUse::Backup);
    }
    Ok(users)
}

/// The Google connector rows for `email`'s account, as `(service, account_email as stored)`. The
/// stored spelling matters: each connector builds its keychain token key from it.
fn google_connector_rows(conn: &Connection, email: &str) -> Result<Vec<(String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT service, account_email FROM connector_sources \
         WHERE provider = 'google' AND account_email IS NOT NULL \
           AND lower(account_email) = lower(?1)",
    )?;
    let rows = stmt
        .query_map(params![email], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<std::result::Result<_, _>>()?;
    Ok(rows)
}

/// The backup account's email exactly as stored, or `None` when backup is off ("" means off).
fn backup_account(conn: &Connection) -> Result<Option<String>> {
    Ok(
        db::get_setting(conn, crate::backup::schedule::BACKUP_GDRIVE_ACCOUNT_KEY)?
            .map(|a| a.trim().to_string())
            .filter(|a| !a.is_empty()),
    )
}

/// The keychain token keys of every grant `email`'s account holds in PM, each built from the spelling
/// its owner saved it under: the connector rows, plus the backup's Drive-key token — which has no
/// connector row when the account backs up without being a Drive connector (a backup-only first
/// connect, or a Drive disconnect that kept the token for backup). De-duplicated.
pub(super) fn google_grant_token_keys(conn: &Connection, email: &str) -> Result<Vec<String>> {
    let mut keys: Vec<String> = google_connector_rows(conn, email)?
        .into_iter()
        .filter_map(|(service, stored)| crate::secrets::token_key_for("google", &service, &stored))
        .collect();
    if let Some(backup) = backup_account(conn)?.filter(|b| b.eq_ignore_ascii_case(email)) {
        keys.push(crate::drive::account_token_key(&backup));
    }
    keys.sort();
    keys.dedup();
    Ok(keys)
}

/// Before an account's own client is saved (or replaced), pin each grant the account already holds
/// (`token_keys`, from [`google_grant_token_keys`]) to the client it refreshes through now, so the
/// new client can't redirect an older token. Per key, by that key's own spelling — the same legacy
/// rule the refresh applies — not by the connect's email. Only tokens saved before minting clients
/// were recorded are touched.
pub(super) async fn pin_existing_grants(token_keys: &[String]) -> Result<()> {
    for key in token_keys {
        crate::google::pin_legacy_token(key).await?;
    }
    Ok(())
}

/// Every spelling a Google account PM signs in to is stored under — the connector rows plus the
/// backup account — de-duplicated exactly, in first-stored order. The candidates for "use the project
/// already saved for this account". Every spelling is kept, not one per person, because an own client
/// saved before 3.139.8 sits under exactly the spelling its connector used, and only that spelling's
/// lookup falls back to it; the caller de-duplicates people after resolving.
pub(super) fn google_account_emails(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT account_email FROM connector_sources \
         WHERE provider = 'google' AND account_email IS NOT NULL ORDER BY created_at",
    )?;
    let mut stored: Vec<String> = stmt
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<std::result::Result<Vec<_>, _>>()?
        .into_iter()
        .map(|e| e.trim().to_string())
        .filter(|e| !e.is_empty())
        .collect();
    stored.extend(backup_account(conn)?);
    let mut seen = BTreeSet::new();
    stored.retain(|e| seen.insert(e.clone()));
    Ok(stored)
}

/// The account a connect should sign in as through its saved project: blank means none.
pub(super) fn saved_project_account(account: Option<String>) -> Option<String> {
    account
        .map(|a| a.trim().to_string())
        .filter(|a| !a.is_empty())
}

/// The error for a sign-in that picked a different account in Google's chooser than the one asked for.
///
/// PM drops that sign-in without revoking it. Google keeps one grant per account and project, so the
/// token it returns belongs to any grant the account already gave this project — on another PM
/// install, say — and a revoke would end that one too; this device can't tell the two apart, so it
/// leaves the choice to the user.
pub(super) fn wrong_account(expected: &str, signed_in_as: &str) -> Error {
    Error::Other(format!(
        "You chose {expected} but signed in as {signed_in_as}. Pick the same account in Google's \
         chooser. PM didn't keep the {signed_in_as} sign-in; if you don't use PM with that \
         account anywhere, you can remove its access at myaccount.google.com/permissions."
    ))
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

// --- changes outside PM ---

/// Whether a command called from the window labelled `label` may change something outside PM (a
/// Google calendar, from #884). Only the main window: PM's commands aren't ACL-gated per window, so
/// the always-on-top briefing window can call any of them, and a write belongs where its Undo, its
/// guest prompt and its errors are shown. Pure, so it's tested without a window.
pub(super) fn caller_may_write(label: &str) -> bool {
    label == "main"
}

/// [`caller_may_write`] as a command guard.
pub(super) fn require_main_window(window: &tauri::Window) -> Result<()> {
    if caller_may_write(window.label()) {
        Ok(())
    } else {
        Err(Error::Other(
            "Calendar changes can only be made from PM's main window.".into(),
        ))
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

    /// Writes outside PM come only from the main window; the briefing window (or any window added
    /// later) is refused by default.
    #[test]
    fn only_the_main_window_may_write() {
        assert!(caller_may_write("main"));
        for label in [crate::tray::BRIEFING_LABEL, "Main", "main ", ""] {
            assert!(!caller_may_write(label), "{label:?}");
        }
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

    /// The candidates for "use the project already saved for this account": each Google account once,
    /// whichever connector spelled it how, plus the backup account; never Outlook or an iCal feed.
    #[test]
    fn google_account_emails_lists_each_google_account_once() {
        let (_dir, conn) = temp_db();
        crate::calendar::upsert_source(
            &conn,
            &crate::calendar::google_account_id("ap@example.com"),
            "google",
            Some("ap@example.com"),
            "ap@example.com",
        )
        .unwrap();
        crate::drive::upsert_account(&conn, "AP@example.com", "AP").unwrap();
        crate::calendar::upsert_source(
            &conn,
            "outlook:work@example.com",
            "microsoft",
            Some("work@example.com"),
            "Work",
        )
        .unwrap();
        crate::calendar::upsert_source(&conn, "ics:abc", "google", None, "Holidays").unwrap();
        db::set_setting(
            &conn,
            crate::backup::schedule::BACKUP_GDRIVE_ACCOUNT_KEY,
            "Backup@Example.com",
        )
        .unwrap();
        // Every stored spelling, once each: an own client saved before 3.139.8 sits under exactly
        // one of them, so the caller resolves each before de-duplicating people.
        assert_eq!(
            google_account_emails(&conn).unwrap(),
            vec!["ap@example.com", "AP@example.com", "Backup@Example.com"]
        );

        // Keychain keys come from each connector's own spelling, since that's what it saved under.
        assert_eq!(
            google_grant_token_keys(&conn, "ap@example.com").unwrap(),
            vec![
                "google_oauth_token_calendar::ap@example.com".to_string(),
                "google_oauth_token_drive::AP@example.com".to_string(),
            ]
        );
        // A backup-only account has no connector row; its Drive-key token is still one of its
        // grants, so saving an own client pins it too.
        assert_eq!(
            google_grant_token_keys(&conn, "backup@example.com").unwrap(),
            vec!["google_oauth_token_drive::Backup@Example.com".to_string()]
        );
    }

    /// Backup on an account that is also a Drive connector signs in through the same Drive-key token,
    /// so it adds no second key.
    #[test]
    fn a_backup_on_a_drive_account_adds_no_second_key() {
        let (_dir, conn) = temp_db();
        crate::drive::upsert_account(&conn, "me@example.com", "Me").unwrap();
        db::set_setting(
            &conn,
            crate::backup::schedule::BACKUP_GDRIVE_ACCOUNT_KEY,
            "me@example.com",
        )
        .unwrap();
        assert_eq!(
            google_grant_token_keys(&conn, "me@example.com").unwrap(),
            vec!["google_oauth_token_drive::me@example.com".to_string()]
        );
    }

    #[test]
    fn a_blank_saved_project_account_means_none() {
        assert_eq!(saved_project_account(None), None);
        assert_eq!(saved_project_account(Some("  ".into())), None);
        assert_eq!(
            saved_project_account(Some(" ap@example.com ".into())).as_deref(),
            Some("ap@example.com")
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
            calendar_write: true,
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"kept_for":["calendar","drive","backup"],"calendar_write":true}"#
        );
    }
}
