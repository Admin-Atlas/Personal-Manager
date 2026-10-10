// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Calendar editing, per Google account (#884): whether PM may change events on an account's
//! calendars, the consent that turns it on, and the switch that turns it off again.
//!
//! Editing is **on** only while three things hold:
//! 1. the user turned it on for the account in PM. That choice is the [`SETTINGS_KEY`] setting, which
//!    only the "Turn on editing" consent sets, and which Disconnect and a read-only reconnect clear;
//! 2. the account's calendar token carries Google's `calendar.events` scope;
//! 3. the user hasn't switched it off in PM ("Turn off editing", local, no round-trip to Google).
//!
//! The choice is the gate, not the scope. Google merges an account's grants within one Cloud project
//! in ways it doesn't document for installed apps, so a token can arrive holding the write scope
//! without anyone asking for it this time. A scope alone never turns editing on.
//!
//! The decisions ([`status`], [`classify_consent`]) are pure and table-tested; the storage around them
//! is one `settings` key.

use std::collections::BTreeMap;

use rusqlite::Connection;
use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::google;

/// `settings` key: `{ "<lowercased account email>": "on" | "paused" }`. An account that isn't listed
/// has editing off. One key for every account, so a change is one read-modify-write under the DB lock.
pub const SETTINGS_KEY: &str = "calendar_editing";

/// What the user chose for an account, as stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Choice {
    On,
    /// Turned on, then switched off in PM. Google still allows it, so switching back needs no consent.
    Paused,
}

/// Whether PM may edit an account's calendars: what Connectors shows, and what every write checks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EditingStatus {
    /// Never turned on, or cleared by a disconnect or a read-only reconnect.
    Off,
    On,
    /// Switched off in PM; Google still allows it.
    Paused,
    /// Turned on, but the account's calendar token no longer carries the write scope (or is gone), so
    /// Google has to be asked again.
    NeedsConsent,
}

/// An account's editing status from the stored choice and its calendar token's granted scopes.
pub fn status(choice: Option<Choice>, token_scope: Option<&str>) -> EditingStatus {
    match choice {
        None => EditingStatus::Off,
        Some(Choice::Paused) => EditingStatus::Paused,
        Some(Choice::On)
            if token_scope
                .is_some_and(|s| google::scope_set_has(s, google::CALENDAR_EVENTS_SCOPE)) =>
        {
            EditingStatus::On
        }
        Some(Choice::On) => EditingStatus::NeedsConsent,
    }
}

/// What a finished "Turn on editing" consent amounts to.
#[derive(Debug, PartialEq, Eq)]
pub enum ConsentVerdict {
    /// The right account granted both scopes: save the token and turn editing on.
    Enabled,
    /// The right account, but the write box was unticked on Google's screen. The token still reads,
    /// so it is saved, and editing stays off.
    WriteDeclined,
    /// A different account signed in. Nothing is saved.
    WrongAccount { signed_in_as: String },
    /// The read box was unticked. The mirror syncs through that scope (and PM can't tell which account
    /// signed in without it), so nothing is saved.
    ReadonlyMissing,
}

/// Classify a consent for `expected` from the scopes Google `granted` and the account it signed in as
/// (`None` when it couldn't be read, which only happens without the read scope).
pub fn classify_consent(
    expected: &str,
    granted: &str,
    signed_in_as: Option<&str>,
) -> ConsentVerdict {
    let Some(signed_in_as) =
        signed_in_as.filter(|_| google::scope_set_has(granted, google::CALENDAR_SCOPE))
    else {
        return ConsentVerdict::ReadonlyMissing;
    };
    if account_key(expected) != account_key(signed_in_as) {
        return ConsentVerdict::WrongAccount {
            signed_in_as: signed_in_as.to_string(),
        };
    }
    if google::scope_set_has(granted, google::CALENDAR_EVENTS_SCOPE) {
        ConsentVerdict::Enabled
    } else {
        ConsentVerdict::WriteDeclined
    }
}

/// Calendar accounts are stored lowercased; the setting follows suit so either spelling finds it.
fn account_key(email: &str) -> String {
    email.trim().to_lowercase()
}

/// Every stored choice. A value PM can't read turns editing off for every account rather than
/// guessing which ones it meant.
pub fn choices(conn: &Connection) -> Result<BTreeMap<String, Choice>> {
    let Some(raw) = crate::db::get_setting(conn, SETTINGS_KEY)? else {
        return Ok(BTreeMap::new());
    };
    Ok(serde_json::from_str(&raw).unwrap_or_default())
}

/// The stored choice for one account.
pub fn choice(conn: &Connection, email: &str) -> Result<Option<Choice>> {
    Ok(lookup(&choices(conn)?, email))
}

/// One account's choice out of [`choices`], whatever the spelling of `email` (an account lifted from
/// the pre-multi-account calendar kept Google's casing).
pub fn lookup(choices: &BTreeMap<String, Choice>, email: &str) -> Option<Choice> {
    choices.get(&account_key(email)).copied()
}

/// Store (or with `None`, clear) one account's choice. Clearing the last one removes the key.
pub fn set_choice(conn: &Connection, email: &str, choice: Option<Choice>) -> Result<()> {
    let mut all = choices(conn)?;
    match choice {
        Some(c) => all.insert(account_key(email), c),
        None => all.remove(&account_key(email)),
    };
    if all.is_empty() {
        return crate::db::delete_setting(conn, SETTINGS_KEY);
    }
    let json =
        serde_json::to_string(&all).map_err(|e| crate::error::Error::Other(e.to_string()))?;
    crate::db::set_setting(conn, SETTINGS_KEY, &json)
}

/// Forget every account's choice (the Google sign-in was cleared, so every Google account went).
pub fn clear_all(conn: &Connection) -> Result<()> {
    crate::db::delete_setting(conn, SETTINGS_KEY)
}

// --- what Google granted, which outlives the choice ---

/// `settings` key: the (lowercased) accounts that granted PM the calendar write scope, as a JSON list.
/// Separate from the choice because the grant outlives it: a read-only reconnect, Turn off editing and
/// Clear client all leave Google's grant as it was, and only a revoke ends it. A Calendar disconnect
/// that keeps the grant for Drive or backups reads it, so the note can say the kept access still
/// covers changing events.
pub const WRITE_GRANTED_KEY: &str = "calendar_write_granted";

fn granted(conn: &Connection) -> Result<std::collections::BTreeSet<String>> {
    let Some(raw) = crate::db::get_setting(conn, WRITE_GRANTED_KEY)? else {
        return Ok(Default::default());
    };
    Ok(serde_json::from_str(&raw).unwrap_or_default())
}

fn store_granted(conn: &Connection, all: &std::collections::BTreeSet<String>) -> Result<()> {
    if all.is_empty() {
        return crate::db::delete_setting(conn, WRITE_GRANTED_KEY);
    }
    let json = serde_json::to_string(all).map_err(|e| crate::error::Error::Other(e.to_string()))?;
    crate::db::set_setting(conn, WRITE_GRANTED_KEY, &json)
}

/// Whether `email`'s account granted PM the calendar write scope and PM hasn't revoked it since.
pub fn write_granted(conn: &Connection, email: &str) -> Result<bool> {
    Ok(granted(conn)?.contains(&account_key(email)))
}

/// Record that `email`'s account granted the calendar write scope.
pub fn mark_write_granted(conn: &Connection, email: &str) -> Result<()> {
    let mut all = granted(conn)?;
    all.insert(account_key(email));
    store_granted(conn, &all)
}

/// PM revoked `email`'s grant at Google, which ends the write scope with everything else.
pub fn forget_write_grant(conn: &Connection, email: &str) -> Result<()> {
    let mut all = granted(conn)?;
    all.remove(&account_key(email));
    store_granted(conn, &all)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::google::{CALENDAR_EVENTS_SCOPE as EVENTS, CALENDAR_SCOPE as READ};

    const DB_KEY: &str = "00112233445566778899aabbccddeeff00112233445566778899aabbccddeeff";

    #[test]
    fn editing_is_on_only_when_chosen_and_granted() {
        use EditingStatus::*;
        let both = format!("{READ} {EVENTS}");
        let cases: [(Option<Choice>, Option<&str>, EditingStatus); 8] = [
            // The scope alone never turns editing on: a merged grant can carry it unasked.
            (None, Some(&both), Off),
            (None, None, Off),
            (Some(Choice::On), Some(&both), On),
            // Chosen, but the token lost the write scope (or the token is gone): ask Google again.
            (Some(Choice::On), Some(READ), NeedsConsent),
            (Some(Choice::On), None, NeedsConsent),
            (Some(Choice::On), Some(""), NeedsConsent),
            // Paused is the user's word, whatever the token holds.
            (Some(Choice::Paused), Some(&both), Paused),
            (Some(Choice::Paused), None, Paused),
        ];
        for (choice, scope, want) in cases {
            assert_eq!(status(choice, scope), want, "{choice:?} {scope:?}");
        }
    }

    #[test]
    fn a_consent_is_classified_before_anything_is_saved() {
        use ConsentVerdict::*;
        let both = format!("{READ} {EVENTS}");
        let cases: [(&str, Option<&str>, ConsentVerdict); 7] = [
            (both.as_str(), Some("me@example.com"), Enabled),
            // Google's casing of the address doesn't matter.
            (both.as_str(), Some("Me@Example.com"), Enabled),
            (READ, Some("me@example.com"), WriteDeclined),
            (
                both.as_str(),
                Some("other@example.com"),
                WrongAccount {
                    signed_in_as: "other@example.com".into(),
                },
            ),
            // Read scope unticked: the account couldn't even be identified.
            (EVENTS, None, ReadonlyMissing),
            (EVENTS, Some("me@example.com"), ReadonlyMissing),
            ("", None, ReadonlyMissing),
        ];
        for (granted, signed_in_as, want) in cases {
            assert_eq!(
                classify_consent("me@example.com", granted, signed_in_as),
                want,
                "{granted:?} {signed_in_as:?}"
            );
        }
    }

    #[test]
    fn choices_are_stored_per_account_and_cleared_cleanly() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        assert_eq!(choice(&conn, "a@example.com").unwrap(), None);

        set_choice(&conn, "A@Example.com", Some(Choice::On)).unwrap();
        set_choice(&conn, "b@example.com", Some(Choice::Paused)).unwrap();
        assert_eq!(choice(&conn, "a@example.com").unwrap(), Some(Choice::On));
        assert_eq!(
            choice(&conn, " B@EXAMPLE.COM ").unwrap(),
            Some(Choice::Paused)
        );
        assert_eq!(
            crate::db::get_setting(&conn, SETTINGS_KEY)
                .unwrap()
                .as_deref(),
            Some(r#"{"a@example.com":"on","b@example.com":"paused"}"#)
        );

        set_choice(&conn, "a@example.com", None).unwrap();
        assert_eq!(choice(&conn, "a@example.com").unwrap(), None);
        set_choice(&conn, "b@example.com", None).unwrap();
        // The last one out removes the key rather than leaving `{}` behind.
        assert_eq!(crate::db::get_setting(&conn, SETTINGS_KEY).unwrap(), None);

        set_choice(&conn, "a@example.com", Some(Choice::On)).unwrap();
        clear_all(&conn).unwrap();
        assert!(choices(&conn).unwrap().is_empty());
    }

    #[test]
    fn an_unreadable_setting_turns_editing_off() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        crate::db::set_setting(&conn, SETTINGS_KEY, r#"{"a@example.com":"sometimes"}"#).unwrap();
        assert_eq!(choice(&conn, "a@example.com").unwrap(), None);
        crate::db::set_setting(&conn, SETTINGS_KEY, "not json").unwrap();
        assert!(choices(&conn).unwrap().is_empty());
    }

    /// The grant outlives the choice: clearing the choice (a read-only reconnect, Clear client) leaves
    /// it, and only forgetting it (PM revoked the grant) removes it.
    #[test]
    fn the_write_grant_outlives_the_choice() {
        let dir = tempfile::tempdir().unwrap();
        let conn = crate::db::open(&dir.path().join("pm.sqlite"), DB_KEY).unwrap();
        assert!(!write_granted(&conn, "a@example.com").unwrap());

        mark_write_granted(&conn, "A@Example.com").unwrap();
        set_choice(&conn, "a@example.com", Some(Choice::On)).unwrap();
        set_choice(&conn, "a@example.com", None).unwrap();
        clear_all(&conn).unwrap();
        assert!(write_granted(&conn, "a@example.com").unwrap());

        forget_write_grant(&conn, " a@example.com ").unwrap();
        assert!(!write_granted(&conn, "a@example.com").unwrap());
        assert_eq!(
            crate::db::get_setting(&conn, WRITE_GRANTED_KEY).unwrap(),
            None
        );
    }
}
