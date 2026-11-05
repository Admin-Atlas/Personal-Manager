// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The ledger's decisions (Mods spec §1.2, §1.3), pure so every refusal is unit-tested: whether this
//! PM serves a package's API range, whether a package is a fresh install, an update or a repair (or
//! refused), the publisher pin, what an update widens, the state a row takes when an install
//! commits, and what a startup check does to a row.

use std::collections::BTreeSet;
use std::fmt;

use serde_json::Value;

use super::disk::PathRefusal;
use super::package::{ApiRange, Manifest, Publisher};
use super::store;

/// The mod API this PM serves, `(major, minor)` (spec §1.5, "API versioning"). Separate from PM's
/// own version; its public name is 1.0. An additive change bumps the minor, a breaking one the major.
pub const MOD_API_VERSION: (u16, u16) = (1, 0);
/// The oldest API major still served: the deprecation window's far end. A mod built only for an
/// older major is parked ("needs an update").
pub const MOD_API_OLDEST_MAJOR: u16 = 1;

/// How a package's API range sits against this PM's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiFit {
    /// Built for this major: runs silently.
    Fits,
    /// Built for an older major still inside the window: runs, with "built for an older PM".
    OlderMajor,
    /// Needs a newer API than this PM has: "update PM".
    NeedsNewerPm,
    /// Built for a major outside the window: parked, "needs an update".
    Retired,
}

impl ApiFit {
    pub fn runs(self) -> bool {
        matches!(self, Self::Fits | Self::OlderMajor)
    }
}

/// Where `range` sits against `host`. `min` is what the mod needs, so a `min` past the host is the
/// only "update PM"; `target` is what it was built for, so its major decides the warning and the
/// window.
pub fn api_fit(range: ApiRange, host: (u16, u16), oldest_major: u16) -> ApiFit {
    if range.min > host {
        ApiFit::NeedsNewerPm
    } else if range.target.0 >= host.0 {
        ApiFit::Fits
    } else if range.target.0 >= oldest_major {
        ApiFit::OlderMajor
    } else {
        ApiFit::Retired
    }
}

/// `M.m`, as a stored manifest's `apiVersion` writes it (the verifier checked it at install).
pub fn parse_major_minor(s: &str) -> Option<(u16, u16)> {
    let (a, b) = s.split_once('.')?;
    let part = |p: &str| -> Option<u16> {
        (!p.is_empty() && p.len() <= 4 && p.bytes().all(|c| c.is_ascii_digit()))
            .then(|| p.parse().ok())
            .flatten()
    };
    Some((part(a)?, part(b)?))
}

/// What an installed row pins: its version and package, and its publisher (spec §1.1: the pin is
/// the publisher, not the key).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pinned {
    pub version: String,
    pub package_sha256: Option<String>,
    pub tier: String,
    pub author: String,
    pub source_repo: String,
}

/// Who made a gift, as its `mod_ledger` row keeps it, or who an installed mod is, as its `mods` row
/// has it (v60).
#[cfg_attr(not(test), allow(dead_code))] // slice 4's "only its own" check reads it
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Maker {
    pub mod_id: String,
    pub signed: bool,
    pub tier: String,
    pub author: String,
    pub source_repo: String,
    pub install_id: String,
}

/// Whether the installed mod `current` may edit or delete, as its own, a task or milestone that
/// `maker` made (spec §1.12: a reinstall can edit its own again). Both are the same mod id, and both
/// signed or both not. A signed maker is the publisher its signature vouched for, so any install of
/// it with the same pin made it. An unsigned maker's pin is only what its own manifest claimed, which
/// any file can copy, so its gifts are that install's alone: the same install id, which a fresh
/// install never keeps (§1.1: an unsigned file with the same id is a fresh install, its grants and
/// secrets reset).
#[cfg_attr(not(test), allow(dead_code))] // slice 4's "only its own" check calls it
pub fn made_it(maker: &Maker, current: &Maker) -> bool {
    if maker.mod_id != current.mod_id || maker.signed != current.signed {
        return false;
    }
    if maker.signed {
        maker.tier == current.tier
            && maker.author == current.author
            && maker.source_repo == current.source_repo
    } else {
        maker.install_id == current.install_id
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallKind {
    Fresh,
    /// A newer version: a new version folder, the old one running until the row commits.
    Update,
    /// Exactly the pinned version and `.pmmod` again, to replace missing or changed files.
    Repair,
}

/// Something an update asks for that the installed version didn't have (spec §1.3, "Updates").
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Widening {
    Permission(String),
    Host(String),
    Secret(String),
    Contribution(String),
}

/// Why the ledger refused a package (or gave up on it part way).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// `ids::check_tier`'s answer.
    Tier(String),
    Path(PathRefusal),
    NeedsNewerPm {
        min: (u16, u16),
    },
    ApiRetired,
    /// Slice 5's index and blacklist checks.
    Catalogue(String),
    /// Another tier, author or source repo than the installed mod's.
    Publisher,
    Downgrade {
        installed: String,
    },
    /// The installed version again, but not the installed file.
    SameVersionDifferentFile,
    /// The installed row can't be read back (a hand-edited store).
    UnreadableRecord,
    /// A background update that asks for more; it waits for the user.
    NeedsApproval(Vec<Widening>),
    /// The row changed between the checks and the commit (another install, or a vault switch).
    Changed,
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Tier(why) => f.write_str(why),
            Self::Path(why) => write!(f, "{why}"),
            Self::NeedsNewerPm { min } => write!(
                f,
                "it needs mod API {}.{}, newer than this PM's {}.{}: update PM to install it",
                min.0, min.1, MOD_API_VERSION.0, MOD_API_VERSION.1
            ),
            Self::ApiRetired => f.write_str(
                "it was built for a mod API this PM no longer serves: it needs an update",
            ),
            Self::Catalogue(why) => f.write_str(why),
            Self::Publisher => f.write_str(
                "it comes from a different publisher than the installed mod with this id",
            ),
            Self::Downgrade { installed } => {
                write!(f, "version {installed} is installed, and this one is older")
            }
            Self::SameVersionDifferentFile => {
                f.write_str("it has the installed version's number but isn't the installed file")
            }
            Self::UnreadableRecord => {
                f.write_str("PM's record of the installed mod can't be read: uninstall it first")
            }
            Self::NeedsApproval(_) => f.write_str("this update asks for more and waits for you"),
            Self::Changed => f.write_str("the installed mods changed while it was installing"),
        }
    }
}

/// A package against the row that pins its id: a fresh install (no row), an update (newer), a
/// repair (the pinned version and `.pmmod` again), or refused.
pub fn decide(
    pinned: Option<&Pinned>,
    version: &semver::Version,
    package_sha256: &str,
    publisher: &Publisher,
) -> Result<InstallKind, Refusal> {
    let Some(p) = pinned else {
        return Ok(InstallKind::Fresh);
    };
    if p.tier != publisher.tier.as_str()
        || p.author != publisher.author
        || p.source_repo != publisher.source_repo
    {
        return Err(Refusal::Publisher);
    }
    let installed = semver::Version::parse(&p.version).map_err(|_| Refusal::UnreadableRecord)?;
    match version.cmp(&installed) {
        std::cmp::Ordering::Greater => Ok(InstallKind::Update),
        std::cmp::Ordering::Equal if p.package_sha256.as_deref() == Some(package_sha256) => {
            Ok(InstallKind::Repair)
        }
        std::cmp::Ordering::Equal => Err(Refusal::SameVersionDifferentFile),
        std::cmp::Ordering::Less => Err(Refusal::Downgrade {
            installed: p.version.clone(),
        }),
    }
}

/// What `new` asks for that the installed manifest (`old`, as stored verbatim) didn't: a permission
/// it didn't declare and that isn't granted, an internet host, a secret slot (or a slot sent to
/// another host or header), or a contribution. A stored manifest that can't be read counts as
/// having asked for nothing, so everything is new.
pub fn widening(old: &str, new: &Manifest, granted: &BTreeSet<String>) -> Vec<Widening> {
    let old: Value = serde_json::from_str(old).unwrap_or(Value::Null);
    let list = |v: &Value| v.as_array().cloned().unwrap_or_default();
    let text = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).map(str::to_owned);
    let old_permissions: BTreeSet<String> = list(&old["permissions"])
        .iter()
        .filter_map(|p| text(p, "id"))
        .collect();
    let old_hosts: BTreeSet<String> = list(&old["internetHosts"])
        .iter()
        .filter_map(|h| h.as_str().map(str::to_owned))
        .collect();
    let old_secrets: BTreeSet<(String, String, String)> = list(&old["secrets"])
        .iter()
        .filter_map(|s| Some((text(s, "name")?, text(s, "host")?, text(s, "header")?)))
        .collect();
    let contributions = &old["contributions"];
    let old_help: BTreeSet<String> = list(&contributions["help"])
        .iter()
        .filter_map(|h| text(h, "id"))
        .collect();

    let mut out = BTreeSet::new();
    for p in &new.permissions {
        if !old_permissions.contains(&p.id) && !granted.contains(&p.id) {
            out.insert(Widening::Permission(p.id.clone()));
        }
    }
    for h in &new.internet_hosts {
        if !old_hosts.contains(h) {
            out.insert(Widening::Host(h.clone()));
        }
    }
    for s in &new.secrets {
        if !old_secrets.contains(&(s.name.clone(), s.host.clone(), s.header.clone())) {
            out.insert(Widening::Secret(s.name.clone()));
        }
    }
    let c = &new.contributions;
    for (name, has) in [
        ("nav.tab", c.nav_tab.is_some()),
        ("calendar.overlay", c.calendar_overlay.is_some()),
    ] {
        if has && contributions.get(name).is_none() {
            out.insert(Widening::Contribution(name.to_owned()));
        }
    }
    for h in &c.help {
        if !old_help.contains(&h.id) {
            out.insert(Widening::Contribution(format!("help:{}", h.id)));
        }
    }
    out.into_iter().collect()
}

/// The state and reason a row takes when an install commits, from what it was (`None`: a fresh
/// install). A pause is the user's, the blacklist's or dev mode's and outlives an update (slice 5
/// decides a resume); a park for a reason the ledger owns (files, record, API) is what an install
/// fixes; any other park (a closed room) is its owner's to lift.
pub fn state_after_install(current: Option<(&str, Option<&str>)>) -> (String, Option<String>) {
    let active = (store::STATE_ACTIVE.to_owned(), None);
    match current {
        None => active,
        Some((state, reason)) if state == store::STATE_PARKED || state == store::STATE_PAUSED => {
            if state == store::STATE_PARKED && reason.is_some_and(store::is_ledger_reason) {
                active
            } else {
                (state.to_owned(), reason.map(str::to_owned))
            }
        }
        Some(_) => active,
    }
}

/// What a startup check does to a row (`None`: leave it). `problem` is the ledger reason its record
/// or files fail with. Only an active row, or one the ledger itself parked, is parked or unparked:
/// a pause, an install in flight and another owner's park are left as they are.
pub fn startup_state(
    state: &str,
    reason: Option<&str>,
    problem: Option<&'static str>,
) -> Option<(&'static str, Option<&'static str>)> {
    let ours = state == store::STATE_PARKED && reason.is_some_and(store::is_ledger_reason);
    match problem {
        Some(p) if state == store::STATE_ACTIVE || ours => (state != store::STATE_PARKED
            || reason != Some(p))
        .then_some((store::STATE_PARKED, Some(p))),
        None if ours => Some((store::STATE_ACTIVE, None)),
        _ => None,
    }
}

/// Whether a row's mod may be served: active, and consented to (an auto-installed pilot's frame
/// waits for its first-open screen, D17).
pub fn serves(state: &str, consented: bool) -> bool {
    state == store::STATE_ACTIVE && consented
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mods::ids::Tier;
    use crate::mods::package::{Contributions, HelpEntry, Permission, SecretSlot};

    fn range(min: (u16, u16), target: (u16, u16)) -> ApiRange {
        ApiRange { min, target }
    }

    #[test]
    fn the_api_range_is_judged_by_its_min_and_its_target_major() {
        let host = (2, 3);
        assert_eq!(api_fit(range((2, 0), (2, 3)), host, 1), ApiFit::Fits);
        assert_eq!(api_fit(range((2, 3), (2, 9)), host, 1), ApiFit::Fits);
        assert_eq!(api_fit(range((1, 0), (3, 0)), host, 1), ApiFit::Fits);
        assert_eq!(
            api_fit(range((2, 4), (2, 4)), host, 1),
            ApiFit::NeedsNewerPm
        );
        assert_eq!(
            api_fit(range((3, 0), (3, 0)), host, 1),
            ApiFit::NeedsNewerPm
        );
        assert_eq!(api_fit(range((1, 0), (1, 9)), host, 1), ApiFit::OlderMajor);
        assert_eq!(api_fit(range((1, 0), (1, 9)), host, 2), ApiFit::Retired);
        assert!(ApiFit::Fits.runs() && ApiFit::OlderMajor.runs());
        assert!(!ApiFit::NeedsNewerPm.runs() && !ApiFit::Retired.runs());
        // Today's PM: 1.0 serves 1.0, and refuses 1.1.
        let today =
            |min, target| api_fit(range(min, target), MOD_API_VERSION, MOD_API_OLDEST_MAJOR);
        assert_eq!(today((1, 0), (1, 0)), ApiFit::Fits);
        assert_eq!(today((1, 1), (1, 1)), ApiFit::NeedsNewerPm);
        assert_eq!(today((0, 9), (0, 9)), ApiFit::Retired);
    }

    #[test]
    fn major_minor_parses_only_its_shape() {
        assert_eq!(parse_major_minor("1.0"), Some((1, 0)));
        assert_eq!(parse_major_minor("12.34"), Some((12, 34)));
        for bad in [
            "", "1", "1.", ".1", "1.0.0", "a.b", "-1.0", "99999.0", "1.0 ",
        ] {
            assert_eq!(parse_major_minor(bad), None, "{bad}");
        }
    }

    fn publisher(tier: Tier, author: &str, repo: &str) -> Publisher {
        Publisher {
            tier,
            author: author.into(),
            source_repo: repo.into(),
            commit: "0".repeat(40),
        }
    }

    fn pinned(version: &str, sha: &str) -> Pinned {
        Pinned {
            version: version.into(),
            package_sha256: Some(sha.into()),
            tier: "first".into(),
            author: "Atlas".into(),
            source_repo: "itsAtlas-org/pm-mods".into(),
        }
    }

    fn v(s: &str) -> semver::Version {
        semver::Version::parse(s).unwrap()
    }

    #[test]
    fn a_package_is_fresh_newer_a_repair_or_refused() {
        let atlas = publisher(Tier::First, "Atlas", "itsAtlas-org/pm-mods");
        let p = pinned("1.2.0", "aa");
        assert_eq!(
            decide(None, &v("0.1.0"), "aa", &atlas),
            Ok(InstallKind::Fresh)
        );
        assert_eq!(
            decide(Some(&p), &v("1.3.0"), "bb", &atlas),
            Ok(InstallKind::Update)
        );
        assert_eq!(
            decide(Some(&p), &v("1.2.1-beta.1"), "bb", &atlas),
            Ok(InstallKind::Update)
        );
        assert_eq!(
            decide(Some(&p), &v("1.2.0"), "aa", &atlas),
            Ok(InstallKind::Repair)
        );
        assert_eq!(
            decide(Some(&p), &v("1.2.0"), "bb", &atlas),
            Err(Refusal::SameVersionDifferentFile)
        );
        assert_eq!(
            decide(Some(&p), &v("1.2.0-rc.1"), "aa", &atlas),
            Err(Refusal::Downgrade {
                installed: "1.2.0".into()
            }),
            "a pre-release of the installed version is older"
        );
        assert_eq!(
            decide(Some(&p), &v("1.1.9"), "aa", &atlas),
            Err(Refusal::Downgrade {
                installed: "1.2.0".into()
            })
        );
        let unsigned_pin = Pinned {
            package_sha256: None,
            ..p.clone()
        };
        assert_eq!(
            decide(Some(&unsigned_pin), &v("1.2.0"), "aa", &atlas),
            Err(Refusal::SameVersionDifferentFile),
            "no pinned file is never a repair"
        );
        let garbled = Pinned {
            version: "one".into(),
            ..p.clone()
        };
        assert_eq!(
            decide(Some(&garbled), &v("2.0.0"), "aa", &atlas),
            Err(Refusal::UnreadableRecord)
        );
    }

    #[test]
    fn an_update_keeps_its_tier_author_and_source_repo() {
        let p = pinned("1.0.0", "aa");
        for other in [
            publisher(Tier::Second, "Atlas", "itsAtlas-org/pm-mods"),
            publisher(Tier::Third, "Atlas", "itsAtlas-org/pm-mods"),
            publisher(Tier::First, "Someone", "itsAtlas-org/pm-mods"),
            publisher(Tier::First, "Atlas", "someone/pm-mods"),
            publisher(Tier::First, "atlas", "itsAtlas-org/pm-mods"),
        ] {
            assert_eq!(
                decide(Some(&p), &v("9.0.0"), "bb", &other),
                Err(Refusal::Publisher),
                "{other:?}"
            );
            assert_eq!(
                decide(Some(&p), &v("1.0.0"), "aa", &other),
                Err(Refusal::Publisher)
            );
        }
    }

    fn manifest(change: impl FnOnce(&mut Manifest)) -> Manifest {
        let mut m = Manifest {
            id: crate::mods::ids::ModId::parse("atlas.map").unwrap(),
            name: "Map".into(),
            description: String::new(),
            icon: None,
            version: v("1.1.0"),
            api: range((1, 0), (1, 0)),
            publisher: publisher(Tier::First, "Atlas", "itsAtlas-org/pm-mods"),
            permissions: vec![
                Permission {
                    id: "storage".into(),
                    why: String::new(),
                },
                Permission {
                    id: "ui.tab".into(),
                    why: String::new(),
                },
            ],
            internet_hosts: vec!["api.example.com".into()],
            secrets: vec![SecretSlot {
                name: "token".into(),
                host: "api.example.com".into(),
                header: "Authorization".into(),
            }],
            contributions: Contributions {
                nav_tab: Some("Map".into()),
                calendar_overlay: None,
                help: vec![HelpEntry {
                    id: "map".into(),
                    title: String::new(),
                    body: String::new(),
                }],
            },
            data: Default::default(),
            entry: crate::mods::package::Entry {
                page: "app/index.html".into(),
                workers: vec![],
            },
            requires: vec![],
        };
        change(&mut m);
        m
    }

    const OLD: &str = r#"{"permissions":[{"id":"storage","why":"x"},{"id":"ui.tab","why":"y"}],
        "internetHosts":["api.example.com"],
        "secrets":[{"name":"token","host":"api.example.com","header":"Authorization"}],
        "contributions":{"nav.tab":{"label":"Map"},"help":[{"id":"map","title":"t","body":"b"}]}}"#;

    #[test]
    fn an_update_that_asks_for_nothing_new_widens_nothing() {
        let none = BTreeSet::new();
        assert_eq!(widening(OLD, &manifest(|_| {}), &none), vec![]);
        // Dropping things is never a widening.
        let fewer = manifest(|m| {
            m.permissions.pop();
            m.internet_hosts.clear();
            m.secrets.clear();
            m.contributions = Contributions::default();
        });
        assert_eq!(widening(OLD, &fewer, &none), vec![]);
    }

    #[test]
    fn add_one_permission_and_drop_one_widens_by_the_one_added() {
        let m = manifest(|m| {
            m.permissions.remove(0);
            m.permissions.push(Permission {
                id: "documents.meta".into(),
                why: String::new(),
            });
        });
        assert_eq!(
            widening(OLD, &m, &BTreeSet::new()),
            vec![Widening::Permission("documents.meta".into())]
        );
        // Already granted (a first-use grant the user gave the old version): not a widening.
        let granted = BTreeSet::from(["documents.meta".to_owned()]);
        assert_eq!(widening(OLD, &m, &granted), vec![]);
    }

    #[test]
    fn a_new_host_secret_or_contribution_widens() {
        let none = BTreeSet::new();
        let host = manifest(|m| m.internet_hosts.push("evil.example.net".into()));
        assert_eq!(
            widening(OLD, &host, &none),
            vec![Widening::Host("evil.example.net".into())]
        );
        let moved = manifest(|m| m.secrets[0].host = "evil.example.net".into());
        assert_eq!(
            widening(OLD, &moved, &none),
            vec![Widening::Secret("token".into())]
        );
        let header = manifest(|m| m.secrets[0].header = "X-Leak".into());
        assert_eq!(
            widening(OLD, &header, &none),
            vec![Widening::Secret("token".into())]
        );
        let overlay = manifest(|m| m.contributions.calendar_overlay = Some("Due".into()));
        assert_eq!(
            widening(OLD, &overlay, &none),
            vec![Widening::Contribution("calendar.overlay".into())]
        );
        let help = manifest(|m| {
            m.contributions.help.push(HelpEntry {
                id: "more".into(),
                title: String::new(),
                body: String::new(),
            })
        });
        assert_eq!(
            widening(OLD, &help, &none),
            vec![Widening::Contribution("help:more".into())]
        );
        // An unreadable stored manifest asked for nothing, so everything is new.
        assert_eq!(widening("not json", &manifest(|_| {}), &none).len(), 6);
    }

    #[test]
    fn an_install_activates_what_the_ledger_parked_and_keeps_every_other_owners_state() {
        let active = ("active".to_owned(), None);
        assert_eq!(state_after_install(None), active);
        assert_eq!(state_after_install(Some(("active", None))), active);
        assert_eq!(state_after_install(Some(("installing", None))), active);
        for reason in store::LEDGER_REASONS {
            assert_eq!(
                state_after_install(Some(("parked", Some(reason)))),
                active,
                "{reason}"
            );
        }
        assert_eq!(
            state_after_install(Some(("parked", Some("room:calendar")))),
            ("parked".to_owned(), Some("room:calendar".to_owned()))
        );
        assert_eq!(
            state_after_install(Some(("paused", Some("blacklist")))),
            ("paused".to_owned(), Some("blacklist".to_owned()))
        );
    }

    #[test]
    fn a_startup_check_parks_and_unparks_only_what_the_ledger_owns() {
        let missing = Some(store::REASON_FILES_MISSING);
        let changed = Some(store::REASON_FILES_CHANGED);
        assert_eq!(startup_state("active", None, None), None);
        assert_eq!(
            startup_state("active", None, missing),
            Some(("parked", missing))
        );
        assert_eq!(
            startup_state("parked", missing, missing),
            None,
            "already parked for it"
        );
        assert_eq!(
            startup_state("parked", missing, changed),
            Some(("parked", changed))
        );
        assert_eq!(
            startup_state("parked", missing, None),
            Some(("active", None)),
            "the files came back"
        );
        assert_eq!(
            startup_state("parked", Some("room:calendar"), missing),
            None
        );
        assert_eq!(startup_state("parked", Some("room:calendar"), None), None);
        assert_eq!(startup_state("parked", None, None), None);
        assert_eq!(startup_state("paused", Some("blacklist"), missing), None);
        assert_eq!(startup_state("installing", None, missing), None);
    }

    #[test]
    fn only_an_active_consented_mod_is_served() {
        assert!(serves("active", true));
        assert!(!serves("active", false));
        for state in ["parked", "paused", "installing"] {
            assert!(!serves(state, true), "{state}");
        }
    }

    /// A signed maker's gifts are its publisher's, across a reinstall; an unsigned maker's are its
    /// own install's, so a file that copies its id and pin (both read from the file itself) gets no
    /// claim on them.
    #[test]
    fn a_gift_is_its_makers_own_by_pin_when_signed_and_by_install_when_not() {
        let maker = |signed: bool, install: &str| Maker {
            mod_id: "acme.tasks".into(),
            signed,
            tier: if signed { "second" } else { "third" }.into(),
            author: "Acme".into(),
            source_repo: "acme/tasks".into(),
            install_id: install.into(),
        };
        let first = "0123456789abcdef";
        let again = "fedcba9876543210";
        // Signed: the same pin from any install, a reinstall included.
        assert!(made_it(&maker(true, first), &maker(true, first)));
        assert!(made_it(&maker(true, first), &maker(true, again)));
        let changes: [fn(&mut Maker); 5] = [
            |m| m.mod_id = "acme.other".into(),
            |m| m.tier = "first".into(),
            |m| m.author = "Not Acme".into(),
            |m| m.source_repo = "someone/tasks".into(),
            |m| m.signed = false,
        ];
        for change in changes {
            let mut current = maker(true, first);
            change(&mut current);
            assert!(!made_it(&maker(true, first), &current), "{current:?}");
        }
        // Unsigned: only the install that made it, however exactly a later file copies the pin.
        assert!(made_it(&maker(false, first), &maker(false, first)));
        assert!(!made_it(&maker(false, first), &maker(false, again)));
        let mut other_id = maker(false, first);
        other_id.mod_id = "acme.other".into();
        assert!(!made_it(&maker(false, first), &other_id));
        // A signed install never inherits an unsigned one's gifts, even keeping its install id.
        let mut signed = maker(true, first);
        signed.tier = "third".into();
        assert!(!made_it(&maker(false, first), &signed));
    }
}
