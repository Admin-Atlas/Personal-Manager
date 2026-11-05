// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The closed value sets of the v60 mod tables (spec §1.12), one owner each (INVARIANTS.md I-12).
//! Every writer takes its strings from here, and a pin test in `db/migrations.rs` reads each set
//! back out of v60's CHECKs, so the two can't drift. `mods.tier` is owned by [`super::ids::Tier`].
//!
//! The ledger (`ledger.rs`: install, update, uninstall, the startup load) and the dispatcher (slice
//! 4) are the writers. Until the Mods tab calls install (slice 6) and the dispatcher lands, some of
//! these have only test readers.
#![cfg_attr(not(test), allow(dead_code))]

use super::package::DataClass;

/// `mods.state`. `installing` until the files are written and verified; `parked` makes no calls
/// (a closed room, missing files, an API major outside the window); `paused` is the user's, the
/// blacklist's or dev mode's (spec §1.3, §1.10, §1.15).
pub const STATES: [&str; 4] = [STATE_ACTIVE, STATE_PAUSED, STATE_PARKED, STATE_INSTALLING];
pub const STATE_ACTIVE: &str = "active";
pub const STATE_PAUSED: &str = "paused";
pub const STATE_PARKED: &str = "parked";
pub const STATE_INSTALLING: &str = "installing";

/// `mods.state_reason` for the parks the ledger itself makes and lifts (`mods/ledger.rs`). Free
/// text in the schema; other owners (a closed room, the blacklist, dev mode) bring their own words
/// and the ledger leaves those rows alone.
pub const REASON_FILES_MISSING: &str = "files-missing";
/// A file is there but isn't what was installed: another size, or not a plain file.
pub const REASON_FILES_CHANGED: &str = "files-changed";
/// The row itself can't be read back into a mod (its id, version, file list or manifest).
pub const REASON_RECORD: &str = "record-unreadable";
/// Its API range is outside what this PM serves (a newer API, or a major past the window).
pub const REASON_API: &str = "api-unsupported";
pub const LEDGER_REASONS: [&str; 4] = [
    REASON_FILES_MISSING,
    REASON_FILES_CHANGED,
    REASON_RECORD,
    REASON_API,
];

pub fn is_ledger_reason(reason: &str) -> bool {
    LEDGER_REASONS.contains(&reason)
}

/// `mods.installed_from`: the signed catalogue, a `.pmmod` file (the Mods tab's "Install from
/// file"), or a developer's unpacked test folder, which has no package sha256 and is left out of
/// backups (§1.15).
pub const INSTALLED_FROM: [&str; 3] = [FROM_CATALOGUE, FROM_FILE, FROM_FOLDER];
pub const FROM_CATALOGUE: &str = "catalogue";
pub const FROM_FILE: &str = "file";
pub const FROM_FOLDER: &str = "folder";

/// `mod_grants.decision`. No row means the permission hasn't been asked yet (a first-use grant).
pub const DECISIONS: [&str; 2] = [GRANTED, DENIED];
pub const GRANTED: &str = "granted";
pub const DENIED: &str = "denied";

/// `mod_kv.class`: the manifest's class for the key. Content is the user's (kept on update, asked
/// about on uninstall, in backups by default, counted as user data); cache is rebuildable and
/// stripped from backups (D4).
pub const KV_CLASSES: [&str; 2] = [KV_CONTENT, KV_CACHE];
pub const KV_CONTENT: &str = "content";
pub const KV_CACHE: &str = "cache";

/// The stored class for a manifest data key's class.
pub fn kv_class(class: DataClass) -> &'static str {
    match class {
        DataClass::Content => KV_CONTENT,
        DataClass::Cache => KV_CACHE,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stored_class_is_the_manifests_word_for_it() {
        // The manifest's `data.keys[].class` and the stored `mod_kv.class` must be one vocabulary,
        // or a key the manifest calls content would be stored under a word the backup strips.
        for word in KV_CLASSES {
            let class: DataClass = serde_json::from_value(serde_json::json!(word)).unwrap();
            assert_eq!(kv_class(class), word);
        }
    }

    #[test]
    fn every_set_is_its_named_values_without_repeats() {
        for set in [
            &STATES[..],
            &INSTALLED_FROM,
            &DECISIONS,
            &KV_CLASSES,
            &LEDGER_REASONS,
        ] {
            let mut seen = std::collections::BTreeSet::new();
            assert!(
                set.iter().all(|v| seen.insert(*v)),
                "{set:?} repeats a value"
            );
        }
    }
}
