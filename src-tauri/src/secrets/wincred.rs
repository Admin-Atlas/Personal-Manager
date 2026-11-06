// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Windows: PM's own Credential Manager writes, so that a secret saved from now on stays on THIS
//! computer.
//!
//! keyring 3 writes every credential with `CRED_PERSIST_ENTERPRISE`, which Windows copies to every
//! computer the user signs in to when their work or school account has a roaming profile. PM's data
//! folder is machine-local (`%LOCALAPPDATA%`, see `paths::data_dir`), and the database key and the
//! tokens belong beside it, so [`set_password`] writes a NEW credential with
//! `CRED_PERSIST_LOCAL_MACHINE` instead. An EXISTING credential is rewritten with the persistence it
//! already has ([`persistence`]): one an earlier PM saved to roam keeps roaming, because another
//! computer the profile roams to may be using it, and taking it off that computer is exactly what
//! must not happen. Nothing is moved, listed or swept, so a secret that roams stops roaming only once
//! it is removed and saved again (disconnecting and reconnecting an account, say).
//!
//! Only the persistence differs from keyring's own write. Each write is shaped exactly as keyring's:
//! the target name, user name, comment and alias are taken from keyring's `WinCredential` for the
//! same entry, and the password is the same UTF-16LE blob with no terminator. So keyring, and every
//! older PM, still reads and deletes them unchanged: `CredReadW` and `CredDeleteW` find a credential
//! by its target name and type, whatever its persistence, which is why reads and deletes stay with
//! keyring. Windows caps, per logon session, how lasting a credential a write may ask for
//! (`CredGetSessionTypes`), and this computer ranks below roaming, so a session that took keyring's
//! roaming writes takes these too.
//!
//! The persistence decision is pure and tested on every platform; the Credential Manager calls are
//! Windows-only, and tested on the `rust-windows` CI job.

// The pure half is only called from the Windows half and from tests.
#![cfg_attr(not(windows), allow(dead_code))]

use zeroize::Zeroizing;

/// `CRED_TYPE_GENERIC`, `CRED_PERSIST_SESSION`, `CRED_PERSIST_LOCAL_MACHINE`,
/// `CRED_PERSIST_ENTERPRISE` and `CRED_MAX_CREDENTIAL_BLOB_SIZE`, restated so the rules below compile
/// and are tested everywhere. Pinned to Windows' own values at compile time on Windows.
const GENERIC: u32 = 1;
const SESSION: u32 = 1;
const THIS_COMPUTER: u32 = 2;
const ROAMING: u32 = 3;
const MAX_BLOB: u32 = 2560;

#[cfg(windows)]
const _: () = {
    use windows_sys::Win32::Security::Credentials as wc;
    assert!(GENERIC == wc::CRED_TYPE_GENERIC);
    assert!(SESSION == wc::CRED_PERSIST_SESSION);
    assert!(THIS_COMPUTER == wc::CRED_PERSIST_LOCAL_MACHINE);
    assert!(ROAMING == wc::CRED_PERSIST_ENTERPRISE);
    assert!(MAX_BLOB == wc::CRED_MAX_CREDENTIAL_BLOB_SIZE);
};

/// What `CredReadW` found under a target, just before a write ([`persistence`] decides from it).
#[derive(Debug)]
enum Found {
    /// No credential there: the read said `ERROR_NOT_FOUND`.
    Nothing,
    /// A credential, saved with this persistence.
    Persist(u32),
    /// The read failed any other way. The text is Windows' error, never the secret.
    Unreadable(String),
}

/// The persistence to write a credential with, from what is [`Found`] under its target.
///
/// One that exists keeps the persistence it has: a roaming one stays roaming, one on this computer
/// stays here, and a session-only one is never made to outlast its session. A new one stays on this
/// computer, unless it takes over a value from another credential (`moved_from`, what was found
/// under that one's target, as when a bundle leftover is split into items): then it takes that one's
/// persistence, by the same rule, so a secret that roamed keeps roaming once it moves. Never a guess:
/// when what is there can't be read, or is saved with a persistence Windows doesn't define, the
/// answer is a refusal, which [`set_password`] turns into a failed write that changes nothing (the
/// text says why).
fn persistence(found: Found, moved_from: Option<Found>) -> Result<u32, String> {
    match found {
        Found::Nothing => match moved_from {
            None => Ok(THIS_COMPUTER),
            Some(from) => persistence(from, None),
        },
        Found::Persist(persist @ (SESSION | THIS_COMPUTER | ROAMING)) => Ok(persist),
        Found::Persist(other) => Err(format!(
            "the saved key is stored with a setting PM doesn't know ({other})"
        )),
        Found::Unreadable(e) => Err(format!("reading how the saved key is stored failed ({e})")),
    }
}

/// A password as keyring stores it on Windows: UTF-16, little-endian, with no terminator. `None` when
/// it is over Credential Manager's blob limit, the same check keyring refuses a write with.
fn utf16le(password: &str) -> Option<Zeroizing<Vec<u8>>> {
    let units = password.encode_utf16().count();
    if units * 2 > MAX_BLOB as usize {
        return None;
    }
    // Sized up front, so the buffer never reallocates and leaves a partial copy behind.
    let mut bytes = Zeroizing::new(Vec::with_capacity(units * 2));
    bytes.extend(password.encode_utf16().flat_map(u16::to_le_bytes));
    Some(bytes)
}

#[cfg(windows)]
pub(super) use win::set_password;

#[cfg(windows)]
mod win {
    use std::iter::once;
    use std::ptr::null_mut;

    use keyring::windows::{decode_error, WinCredential};
    use keyring::{Entry, Error, Result};
    use windows_sys::Win32::Security::Credentials::{CredFree, CredReadW, CredWriteW, CREDENTIALW};
    use zeroize::Zeroize;

    use super::{persistence, utf16le, Found, GENERIC, MAX_BLOB};

    /// keyring's `Entry::set_password`, except for the persistence ([`persistence`]): a credential
    /// new under this target stays on this computer, or takes `moved_from`'s persistence when it
    /// takes over that entry's value; one already there keeps its own.
    ///
    /// The credential already there is read just before the write (only its persistence is used,
    /// and its secret is wiped from this process's copy). Within PM no other write lands in between,
    /// since every one runs under `secrets.rs`'s cache lock. The wipe's deletes don't take it, but a
    /// delete in between only means this write makes the credential again, as keyring's own write
    /// would, with the persistence it had a moment before. A refused write, an oversize one
    /// included, changes nothing, and so does a failed one: `CredWriteW` replaces a credential in one
    /// step and never deletes first.
    pub fn set_password(entry: &Entry, password: &str, moved_from: Option<&Entry>) -> Result<()> {
        let Some(cred) = win_credential(entry) else {
            // Not a Credential Manager entry (keyring's mock store), so there is no persistence to set.
            return entry.set_password(password);
        };
        let Some(mut blob) = utf16le(password) else {
            return Err(Error::TooLong(
                "password encoded as UTF-16".to_string(),
                MAX_BLOB,
            ));
        };
        let found = look_up(&cred.target_name);
        // The other credential matters only to a new one, so it is read only then.
        let from = match (&found, moved_from.and_then(win_credential)) {
            (Found::Nothing, Some(from)) => Some(look_up(&from.target_name)),
            _ => None,
        };
        let persist = persistence(found, from).map_err(|why| {
            Error::PlatformFailure(
                format!("PM didn't save the key, so what's saved is unchanged: {why}").into(),
            )
        })?;
        write_as(cred, &mut blob, persist)
    }

    fn win_credential(entry: &Entry) -> Option<&WinCredential> {
        entry.get_credential().downcast_ref::<WinCredential>()
    }

    /// What `CredReadW`, the read keyring's own reads make, finds under `target`.
    fn look_up(target: &str) -> Found {
        match Read::of(target) {
            Ok(read) => Found::Persist(read.record().Persist),
            Err(Error::NoEntry) => Found::Nothing,
            Err(e) => Found::Unreadable(e.to_string()),
        }
    }

    /// Write `blob` as `cred`'s generic credential, with `persist`.
    pub(super) fn write_as(cred: &WinCredential, blob: &mut [u8], persist: u32) -> Result<()> {
        let mut target = wide(&cred.target_name);
        let mut comment = wide(&cred.comment);
        let mut alias = wide(&cred.target_alias);
        let mut user = wide(&cred.username);
        let record = CREDENTIALW {
            Type: GENERIC,
            TargetName: target.as_mut_ptr(),
            Comment: comment.as_mut_ptr(),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: persist,
            TargetAlias: alias.as_mut_ptr(),
            UserName: user.as_mut_ptr(),
            // No flags, no attributes, and a LastWritten that CredWriteW ignores: keyring's own.
            ..Default::default()
        };
        // SAFETY: every pointer in `record` is live for the call, which only reads through them.
        if unsafe { CredWriteW(&record, 0) } == 0 {
            return Err(decode_error());
        }
        Ok(())
    }

    /// One generic credential as `CredReadW` returns it; its secret is wiped and it is freed on drop.
    pub(super) struct Read(*mut CREDENTIALW);

    impl Read {
        pub(super) fn of(target: &str) -> Result<Self> {
            let name = wide(target);
            let mut cred: *mut CREDENTIALW = null_mut();
            // SAFETY: `name` is NUL-terminated; on success Windows allocates `cred`, freed in Drop.
            if unsafe { CredReadW(name.as_ptr(), GENERIC, 0, &mut cred) } == 0 {
                return Err(decode_error());
            }
            Ok(Self(cred))
        }

        pub(super) fn record(&self) -> &CREDENTIALW {
            // SAFETY: a successful CredReadW's allocation, live until Drop.
            unsafe { &*self.0 }
        }
    }

    impl Drop for Read {
        fn drop(&mut self) {
            // Zero the secret in this process's copy, as keyring does before freeing one.
            let cred = self.record();
            if !cred.CredentialBlob.is_null() && cred.CredentialBlobSize != 0 {
                // SAFETY: Windows' blob pointer and size, in memory Windows allocated for this process.
                unsafe {
                    std::slice::from_raw_parts_mut(
                        cred.CredentialBlob,
                        cred.CredentialBlobSize as usize,
                    )
                }
                .zeroize();
            }
            // SAFETY: allocated by CredReadW in `of`, freed once.
            unsafe { CredFree(self.0 as *const _) };
        }
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(once(0)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_credential_stays_on_this_computer() {
        assert_eq!(persistence(Found::Nothing, None), Ok(THIS_COMPUTER));
    }

    #[test]
    fn an_existing_credential_keeps_the_persistence_it_has() {
        assert_eq!(
            persistence(Found::Persist(ROAMING), None),
            Ok(ROAMING),
            "one an earlier PM saved to roam keeps roaming, so no other computer loses it"
        );
        assert_eq!(
            persistence(Found::Persist(THIS_COMPUTER), None),
            Ok(THIS_COMPUTER)
        );
        assert_eq!(
            persistence(Found::Persist(SESSION), None),
            Ok(SESSION),
            "a session-only one is never made to outlast its session"
        );
    }

    #[test]
    fn a_write_that_cant_tell_what_is_there_is_refused_not_guessed() {
        let why = persistence(Found::Unreadable("Access is denied.".into()), None).unwrap_err();
        assert!(
            why.contains("reading how the saved key is stored failed")
                && why.contains("Access is denied."),
            "{why}"
        );
        // Windows defines only 1-3; anything else is not written back, or replaced, on a guess.
        for unknown in [0, 4, u32::MAX] {
            let why = persistence(Found::Persist(unknown), None).unwrap_err();
            assert!(why.contains(&format!("({unknown})")), "{why}");
        }
    }

    #[test]
    fn a_moved_value_takes_the_persistence_of_the_credential_it_came_from() {
        // New under its own name: it takes over the other's persistence, by the same rule.
        for persist in [ROAMING, THIS_COMPUTER, SESSION] {
            assert_eq!(
                persistence(Found::Nothing, Some(Found::Persist(persist))),
                Ok(persist)
            );
        }
        assert_eq!(
            persistence(Found::Nothing, Some(Found::Nothing)),
            Ok(THIS_COMPUTER),
            "nothing there either: new, so it stays on this computer"
        );
        assert!(persistence(Found::Nothing, Some(Found::Unreadable("E".into()))).is_err());
        assert!(persistence(Found::Nothing, Some(Found::Persist(7))).is_err());

        // Already there under its own name: its own persistence wins, whatever the other's.
        assert_eq!(
            persistence(Found::Persist(THIS_COMPUTER), Some(Found::Persist(ROAMING))),
            Ok(THIS_COMPUTER)
        );
        assert_eq!(
            persistence(Found::Persist(ROAMING), Some(Found::Persist(THIS_COMPUTER))),
            Ok(ROAMING)
        );
        assert!(
            persistence(Found::Unreadable("E".into()), Some(Found::Persist(ROAMING))).is_err(),
            "its own unreadable is refused, never settled by the other's"
        );
    }

    #[test]
    fn a_password_is_encoded_exactly_as_keyring_stores_it() {
        // UTF-16LE with no terminator: what keyring writes, and so what every older PM reads.
        assert_eq!(
            utf16le("a\u{e9}\u{1f600}").unwrap().as_slice(),
            [0x61, 0x00, 0xe9, 0x00, 0x3d, 0xd8, 0x00, 0xde]
        );
        assert!(utf16le("").unwrap().is_empty());
    }

    #[test]
    fn the_blob_limit_counts_utf16_units_as_keyring_does() {
        // 2560 bytes is 1280 UTF-16 units, whatever the characters are.
        assert!(utf16le(&"x".repeat(1280)).is_some());
        assert!(utf16le(&"x".repeat(1281)).is_none());
        // An emoji is two units (four bytes): 640 fit, 641 don't.
        assert!(utf16le(&"\u{1f600}".repeat(640)).is_some());
        assert!(utf16le(&"\u{1f600}".repeat(641)).is_none());
        // é is one unit (two UTF-8 bytes): 1280 fit.
        assert!(utf16le(&"\u{e9}".repeat(1280)).is_some());
    }
}

/// Against the real Credential Manager, on the `rust-windows` CI job. Every test works under a
/// service of its own, unique to the run, so it can never see or touch PM's real entries, and it
/// deletes what it wrote even when it fails.
#[cfg(all(test, windows))]
mod windows_tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use keyring::windows::WinCredential;
    use keyring::{Entry, Error};

    use super::win::{write_as, Read};
    use super::*;

    fn test_service() -> String {
        static N: AtomicUsize = AtomicUsize::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        format!(
            "pm-test-{}-{nanos}-{}.invalid",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        )
    }

    /// Deletes the named entries on drop, so a failed assertion leaves nothing behind.
    struct Cleanup(Vec<(String, String)>);

    impl Drop for Cleanup {
        fn drop(&mut self) {
            for (service, name) in &self.0 {
                if let Ok(entry) = Entry::new(service, name) {
                    let _ = entry.delete_credential();
                }
            }
        }
    }

    fn persist_of(service: &str, name: &str) -> u32 {
        Read::of(&format!("{name}.{service}"))
            .expect("the credential should be there")
            .record()
            .Persist
    }

    #[test]
    fn a_new_credential_stays_on_this_computer_and_keyring_reads_it() {
        let service = test_service();
        let name = "db_encryption_key";
        let _cleanup = Cleanup(vec![(service.clone(), name.to_string())]);
        let entry = Entry::new(&service, name).unwrap();

        set_password(&entry, "value-under-test-\u{e9}\u{1f600}", None).unwrap();
        assert_eq!(persist_of(&service, name), THIS_COMPUTER);
        assert_eq!(
            entry.get_password().unwrap(),
            "value-under-test-\u{e9}\u{1f600}",
            "keyring, and so every older PM, reads it"
        );
        // keyring's own attributes, so the credential looks as keyring would have left it.
        let attrs = entry.get_attributes().unwrap();
        assert_eq!(attrs["username"], name);
        assert!(attrs["comment"].starts_with("keyring v"));

        set_password(&entry, "second", None).unwrap();
        assert_eq!(
            persist_of(&service, name),
            THIS_COMPUTER,
            "an overwrite stays here too"
        );
        assert_eq!(entry.get_password().unwrap(), "second");

        entry.delete_credential().unwrap();
        assert!(matches!(entry.get_password(), Err(Error::NoEntry)));
    }

    #[test]
    fn a_credential_saved_to_roam_keeps_roaming_when_pm_rewrites_it() {
        let service = test_service();
        let name = "google_oauth_token_drive::Someone.Else@example.com";
        let _cleanup = Cleanup(vec![(service.clone(), name.to_string())]);
        let entry = Entry::new(&service, name).unwrap();

        // Written the old way: keyring's own write, which roams.
        entry.set_password("old").unwrap();
        assert_eq!(persist_of(&service, name), ROAMING, "precondition");

        set_password(&entry, "new", None).unwrap();
        assert_eq!(
            persist_of(&service, name),
            ROAMING,
            "another computer the profile roams to may be using it"
        );
        assert_eq!(entry.get_password().unwrap(), "new");
        assert_eq!(entry.get_attributes().unwrap()["username"], name);
    }

    #[test]
    fn a_session_only_credential_stays_session_only() {
        let service = test_service();
        let name = "backup_passphrase";
        let _cleanup = Cleanup(vec![(service.clone(), name.to_string())]);
        let entry = Entry::new(&service, name).unwrap();
        let cred = entry
            .get_credential()
            .downcast_ref::<WinCredential>()
            .unwrap();
        write_as(cred, &mut utf16le("old").unwrap(), SESSION).unwrap();
        assert_eq!(persist_of(&service, name), SESSION, "precondition");

        set_password(&entry, "new", None).unwrap();
        assert_eq!(persist_of(&service, name), SESSION);
        assert_eq!(entry.get_password().unwrap(), "new");
    }

    #[test]
    fn an_oversize_write_is_refused_and_changes_nothing() {
        let service = test_service();
        let (kept, absent) = ("db_encryption_key", "openrouter_api_key");
        let _cleanup = Cleanup(vec![
            (service.clone(), kept.to_string()),
            (service.clone(), absent.to_string()),
        ]);
        let over = "x".repeat(1281);

        // Over one already there: refused, and it still reads and is stored as before.
        let entry = Entry::new(&service, kept).unwrap();
        entry.set_password("kept").unwrap();
        assert!(matches!(
            set_password(&entry, &over, None),
            Err(Error::TooLong(..))
        ));
        assert_eq!(entry.get_password().unwrap(), "kept");
        assert_eq!(persist_of(&service, kept), ROAMING);

        // Where there is none: refused, and none is made.
        let entry = Entry::new(&service, absent).unwrap();
        assert!(matches!(
            set_password(&entry, &over, None),
            Err(Error::TooLong(..))
        ));
        assert!(matches!(entry.get_password(), Err(Error::NoEntry)));
    }

    #[test]
    fn a_moved_value_takes_the_persistence_of_the_credential_it_came_from() {
        let service = test_service();
        let names = ["pm_secrets", "db_encryption_key", "openrouter_api_key"];
        let missing = "never_written";
        let _cleanup = Cleanup(
            names
                .iter()
                .chain([&missing])
                .map(|n| (service.clone(), n.to_string()))
                .collect(),
        );
        let [bundle, key, api] = names.map(|n| Entry::new(&service, n).unwrap());

        // A bundle left by an earlier PM, written the old way, so it roams.
        bundle.set_password("{}").unwrap();
        assert_eq!(persist_of(&service, names[0]), ROAMING, "precondition");

        // New under its own name: it roams like the bundle it came from.
        set_password(&key, "moved", Some(&bundle)).unwrap();
        assert_eq!(persist_of(&service, names[1]), ROAMING);
        assert_eq!(key.get_password().unwrap(), "moved");

        // Already here under its own name: it keeps its own.
        set_password(&api, "here", None).unwrap();
        set_password(&api, "moved", Some(&bundle)).unwrap();
        assert_eq!(persist_of(&service, names[2]), THIS_COMPUTER);
        assert_eq!(api.get_password().unwrap(), "moved");

        // From one that isn't there: new, so it stays on this computer.
        key.delete_credential().unwrap();
        let gone = Entry::new(&service, missing).unwrap();
        set_password(&key, "moved", Some(&gone)).unwrap();
        assert_eq!(persist_of(&service, names[1]), THIS_COMPUTER);
    }
}
