// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Mod ids and trust tiers (Mods spec §1.1, §1.2).
//!
//! A [`ModId`] is checked once, here, before it reaches a path, a header, a URL or a `source_id`;
//! everything downstream takes the type, never a bare string. The grammar is lowercase ASCII only,
//! so two ids can't differ only by case and collide on NTFS or APFS, and no segment may be a
//! Windows device name, which Windows resolves even inside a folder and with an extension.

use std::fmt;

use serde::Serialize;

/// Segments in an id: a publisher-ish first one and one to four more.
const MIN_SEGMENTS: usize = 2;
const MAX_SEGMENTS: usize = 5;
const MAX_SEGMENT_LEN: usize = 31;

/// The prefix first party owns (spec §1.1).
const RESERVED_PREFIX: &[u8] = b"atlas.";

/// Why an id or a tier pairing was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdError {
    /// Not `^[a-z][a-z0-9-]{0,30}(\.[a-z][a-z0-9-]{0,30}){1,4}$`.
    Grammar,
    /// A segment is a Windows device name (`con`, `nul`, `com1`…).
    DeviceName,
    /// An `atlas.` id from anyone but first party.
    Reserved,
    /// First party outside `atlas.`.
    FirstPartyOutsideAtlas,
}

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Grammar => {
                "a mod id is two to five dot-separated parts, each a lowercase letter then up to 30 \
                 lowercase letters, digits or hyphens"
            }
            Self::DeviceName => "a mod id can't use a name Windows reserves for a device",
            Self::Reserved => "only Atlas's own mods may use an id starting \"atlas.\"",
            Self::FirstPartyOutsideAtlas => "an Atlas mod's id must start \"atlas.\"",
        })
    }
}

/// A validated mod id. The only way to make one is [`ModId::parse`].
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct ModId(String);

impl ModId {
    pub fn parse(s: &str) -> Result<Self, IdError> {
        let segments: Vec<&str> = s.split('.').collect();
        if !(MIN_SEGMENTS..=MAX_SEGMENTS).contains(&segments.len()) {
            return Err(IdError::Grammar);
        }
        for segment in &segments {
            let b = segment.as_bytes();
            let shaped = !b.is_empty()
                && b.len() <= MAX_SEGMENT_LEN
                && b[0].is_ascii_lowercase()
                && b.iter()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-');
            if !shaped {
                return Err(IdError::Grammar);
            }
        }
        if segments.iter().any(|s| is_device_name(s)) {
            return Err(IdError::DeviceName);
        }
        Ok(Self(s.to_owned()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether the id is in first party's `atlas.` space.
    pub fn is_reserved(&self) -> bool {
        is_reserved(&self.0)
    }
}

impl fmt::Display for ModId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// `atlas.`, compared case-insensitively (spec §1.1), so a check made before the grammar (or by a
/// caller holding an unvalidated string) can't be dodged with `Atlas.`.
pub fn is_reserved(id: &str) -> bool {
    let b = id.as_bytes();
    b.len() >= RESERVED_PREFIX.len()
        && b[..RESERVED_PREFIX.len()].eq_ignore_ascii_case(RESERVED_PREFIX)
}

/// Whether a path component is a name Windows maps to a device: `con`, `prn`, `aux`, `nul`,
/// `com0`-`com9`, `lpt0`-`lpt9` (and the console's `conin$`, `conout$`), in any case and with any
/// extension (`nul.txt`, `com1.tar.gz`), since Windows ignores everything from the first dot.
/// Refused on every OS, so a package means the same thing everywhere.
pub fn is_device_name(component: &str) -> bool {
    let stem = component.split('.').next().unwrap_or(component);
    let lower = stem.to_ascii_lowercase();
    match lower.as_str() {
        "con" | "prn" | "aux" | "nul" | "conin$" | "conout$" => true,
        _ => {
            let b = lower.as_bytes();
            b.len() == 4
                && (b.starts_with(b"com") || b.starts_with(b"lpt"))
                && b[3].is_ascii_digit()
        }
    }
}

/// Who published a mod (spec §1.1). First and second party are signed by an Atlas key; third
/// party is unsigned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Tier {
    First,
    Second,
    Third,
}

impl Tier {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "first" => Some(Self::First),
            "second" => Some(Self::Second),
            "third" => Some(Self::Third),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::First => "first",
            Self::Second => "second",
            Self::Third => "third",
        }
    }
}

/// Whether `tier` may publish `id`: `atlas.` belongs to first party, and first party publishes only
/// there (spec §1.1's table). The verifier calls it for every signed package, and every install
/// path calls it again. A dev build's unsigned `atlas.` folder (§1.1, last bullet) is a separate,
/// explicit exception that path makes, never this function.
pub fn check_tier(id: &ModId, tier: Tier) -> Result<(), IdError> {
    match (id.is_reserved(), tier) {
        (true, Tier::First) | (false, Tier::Second | Tier::Third) => Ok(()),
        (true, Tier::Second | Tier::Third) => Err(IdError::Reserved),
        (false, Tier::First) => Err(IdError::FirstPartyOutsideAtlas),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_formed_ids_parse() {
        for id in [
            "atlas.map",
            "atlas.pinboard",
            "a.b",
            "acme.tools-2.x9",
            "a.b.c.d.e",
            "console.app", // a device name only as the whole stem
            "com10.thing", // com + two digits isn't a device
            "lpt.x",
            "nulls.x",
            &format!("{}.{}", "a".repeat(31), "b".repeat(31)),
        ] {
            let parsed = ModId::parse(id).unwrap_or_else(|e| panic!("{id}: {e}"));
            assert_eq!(parsed.as_str(), id);
        }
    }

    #[test]
    fn ids_outside_the_grammar_are_refused() {
        for id in [
            "",
            "atlas",
            ".",
            "atlas.",
            ".map",
            "atlas..map",
            "atlas.map.",
            "atlas.map;rm",
            "atlas.map x",
            "atlas map",
            "atlas.map\n",
            "atlas.map\0",
            "..",
            "../atlas.map",
            "atlas/map",
            "atlas\\map",
            "atlas:map",
            "Atlas.map",
            "atlas.Map",
            "ATLAS.MAP",
            "atlas.mäp",
            "atlas.map\u{200b}",
            "аtlas.map", // Cyrillic а
            "1atlas.map",
            "atlas.1map",
            "atlas.-map",
            "atlas._map",
            "a.b.c.d.e.f",
            &format!("{}.b", "a".repeat(32)),
            &format!("a.{}", "b".repeat(32)),
            "atlas.map%2e",
            "atlas.map?x",
        ] {
            assert_eq!(ModId::parse(id), Err(IdError::Grammar), "{id:?}");
        }
    }

    #[test]
    fn device_names_are_refused_in_any_segment() {
        for id in [
            "con.app", "app.con", "prn.x", "aux.x", "nul.x", "a.b.nul", "com0.x", "com1.x",
            "com9.x", "lpt0.x", "lpt5.x", "x.lpt9",
        ] {
            assert_eq!(ModId::parse(id), Err(IdError::DeviceName), "{id}");
        }
    }

    #[test]
    fn device_names_match_with_any_case_and_extension() {
        for name in [
            "con",
            "CON",
            "Con",
            "nul.txt",
            "NUL.tar.gz",
            "com1",
            "COM1.js",
            "lpt0.x",
            "aux.",
            "prn.html",
            "conin$",
            "CONOUT$.x",
        ] {
            assert!(is_device_name(name), "{name}");
        }
        for name in [
            "console", "com", "com10", "lpt", "nul-1", "xnul", ".con", "a.con", "comx", "",
        ] {
            assert!(!is_device_name(name), "{name}");
        }
    }

    #[test]
    fn atlas_is_first_partys_and_first_party_is_atlas_only() {
        let atlas = ModId::parse("atlas.map").unwrap();
        let other = ModId::parse("acme.map").unwrap();
        assert_eq!(check_tier(&atlas, Tier::First), Ok(()));
        assert_eq!(check_tier(&atlas, Tier::Second), Err(IdError::Reserved));
        assert_eq!(check_tier(&atlas, Tier::Third), Err(IdError::Reserved));
        assert_eq!(check_tier(&other, Tier::Second), Ok(()));
        assert_eq!(check_tier(&other, Tier::Third), Ok(()));
        assert_eq!(
            check_tier(&other, Tier::First),
            Err(IdError::FirstPartyOutsideAtlas)
        );
        // A prefix, not a segment match: `atlasx.map` is anyone's.
        assert!(!ModId::parse("atlasx.map").unwrap().is_reserved());
    }

    #[test]
    fn the_reserved_check_ignores_case_for_unvalidated_strings() {
        for id in ["atlas.map", "Atlas.map", "ATLAS.x", "aTlAs."] {
            assert!(is_reserved(id), "{id}");
        }
        for id in ["atlas", "atlasmap", "xatlas.map", "", "atla"] {
            assert!(!is_reserved(id), "{id}");
        }
    }

    #[test]
    fn tiers_round_trip_and_nothing_else_parses() {
        for t in [Tier::First, Tier::Second, Tier::Third] {
            assert_eq!(Tier::parse(t.as_str()), Some(t));
        }
        for s in ["First", "", "fourth", " first", "first "] {
            assert_eq!(Tier::parse(s), None, "{s:?}");
        }
    }
}
