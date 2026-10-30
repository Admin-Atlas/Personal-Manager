// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Asking the owner of a Google calendar shared with you to view only for edit access (plan Q10,
//! A12): whom to ask, and the email PM opens in the user's mail app. Google has no API for it. Pure;
//! nothing here opens or sends anything.
//!
//! The webview names only PM's calendar id: whom to ask comes from the stored calendar list (Google's
//! `dataOwner`, else a primary calendar's id, which is its owner's address), every byte of the email
//! outside the unreserved set is percent-encoded, and a calendar's name stays on one line, so a name
//! or an owner Google sent can't add a recipient, a field or a line to it.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use serde::Serialize;

use crate::calendar::Calendar;

/// Whom to ask for edit access to one calendar, for the calendar view.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AskOwner {
    /// The owner's address, as Google gave it.
    pub owner: String,
    /// The owner is one of the Google accounts connected to PM: the view says how to share it from
    /// there, rather than opening an email to themselves.
    pub connected: bool,
}

/// Longest calendar name the email repeats, in characters; a longer one is cut with "…". Keeps the
/// link under 2,000 characters, which every system's opener takes.
pub const NAME_MAX: usize = 40;

/// The permission the email asks for, by the name Google's sharing settings give it (Help 37082).
const PERMISSION: &str = "Make changes and see event details";

/// Whether `text` is an address PM may put in an email: trimmed, at most 254 characters, one `@`, a
/// local part (1-64) of letters, digits and `. _ % + - '`, and a domain of two or more dot-separated
/// labels of letters, digits and `-` (none empty, none starting or ending with `-`). Stricter than
/// Google's calendar ids on purpose: a holiday calendar's `en.usa#holiday@…` fails it.
pub fn is_plain_address(text: &str) -> bool {
    let Some((local, domain)) = text.split_once('@') else {
        return false;
    };
    let label = |l: &str| {
        !l.is_empty()
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    };
    text.len() <= 254
        && (1..=64).contains(&local.len())
        && local
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._%+-'".contains(c))
        && domain.split('.').count() >= 2
        && domain.split('.').all(label)
}

/// Whether `address` is on one of Google Calendar's own domains (`calendar.google.com` or a
/// subdomain: `group.`, `group.v.`, `import.`, `resource.`): a calendar, not a person.
/// Case-insensitive.
pub fn on_calendar_domain(address: &str) -> bool {
    let domain = address
        .rsplit_once('@')
        .map_or(address, |(_, d)| d)
        .to_ascii_lowercase();
    domain == "calendar.google.com" || domain.ends_with(".calendar.google.com")
}

/// Whom to ask about `calendar`, or `None`: a Google calendar whose role is `reader` or
/// `freeBusyReader` (not a missing or unknown role), owned by `dataOwner` when Google sent one, else
/// by its id when that is a person's address (a primary calendar's id is its owner's). A `dataOwner`
/// that isn't a plain address, or is on a calendar domain, gives `None`: PM doesn't guess.
pub fn owner_to_ask(calendar: &Calendar) -> Option<String> {
    if calendar.provider != "google"
        || !matches!(
            calendar.facts.access_role.as_deref(),
            Some("reader" | "freeBusyReader")
        )
    {
        return None;
    }
    let owner = match &calendar.facts.data_owner {
        Some(named) => named,
        None => calendar.remote_id.as_ref()?,
    };
    (is_plain_address(owner) && !on_calendar_domain(owner)).then(|| owner.clone())
}

/// Whether `owner` is one of the `connected` addresses (Google writes an address's case as it
/// likes).
fn is_connected(owner: &str, connected: &[&str]) -> bool {
    connected.iter().any(|c| c.eq_ignore_ascii_case(owner))
}

/// Every calendar's [`AskOwner`], keyed by PM's calendar id, for the overview. `connected` holds the
/// addresses of the Google accounts connected to PM.
pub fn ask_map(calendars: &[Calendar], connected: &[&str]) -> BTreeMap<String, AskOwner> {
    calendars
        .iter()
        .filter_map(|c| {
            let owner = owner_to_ask(c)?;
            let connected = is_connected(&owner, connected);
            Some((c.id.clone(), AskOwner { owner, connected }))
        })
        .collect()
}

/// What asking about `calendar_id` comes to: the owner and the email's link, or `None` when there's
/// nobody to ask now (the calendar is gone or no longer view-only, its owner is connected to PM, or
/// PM has no address for the calendar's own Google sign-in). `addresses` pairs each connected Google
/// source's id with its address. The command's whole decision, so it is tested here.
pub fn ask_plan(
    calendars: &[Calendar],
    addresses: &[(&str, &str)],
    calendar_id: &str,
) -> Option<(String, String)> {
    let calendar = calendars.iter().find(|c| c.id == calendar_id)?;
    let owner = owner_to_ask(calendar)?;
    let connected: Vec<&str> = addresses.iter().map(|(_, a)| *a).collect();
    if is_connected(&owner, &connected) {
        return None;
    }
    let own = addresses
        .iter()
        .find(|(id, _)| *id == calendar.source_id)
        .map(|(_, a)| a.trim())
        .filter(|a| !a.is_empty())?;
    let link = mailto(&owner, own, &calendar.name);
    Some((owner, link))
}

/// `text` percent-encoded as UTF-8: every byte but `A-Z a-z 0-9 - . _ ~` (and `@` where `at` is
/// kept), so nothing in it can end the field it is in or start another. Hand-written because
/// `form_urlencoded` writes a space as `+`, which mail apps show as it is.
fn encoded(text: &str, at: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for &b in text.as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') || (at && b == b'@')
        {
            out.push(char::from(b));
        } else {
            let _ = write!(out, "%{b:02X}");
        }
    }
    out
}

/// A calendar's name for the email, on one line (whoever made the calendar named it: a line break in
/// the subject could read as another header), and at most [`NAME_MAX`] characters, a longer one cut
/// with "…".
fn short_name(name: &str) -> String {
    let one_line = name.chars().map(|c| if c.is_control() { ' ' } else { c });
    if name.chars().count() <= NAME_MAX {
        one_line.collect()
    } else {
        one_line.take(NAME_MAX).chain(['…']).collect()
    }
}

/// The `mailto:` link for asking `owner` to let `own` change `calendar_name`: RFC 6068, every byte
/// but `A-Z a-z 0-9 - . _ ~` percent-encoded as UTF-8 (`@` kept in the address), line breaks as
/// `%0D%0A`. `+` is never a space here.
pub fn mailto(owner: &str, own: &str, calendar_name: &str) -> String {
    let name = short_name(calendar_name);
    let subject = format!("Edit access to your calendar “{name}”");
    let body = [
        "Hi,".to_string(),
        String::new(),
        format!("Could you let {own} make changes to your Google calendar “{name}”?"),
        String::new(),
        format!(
            "In Google Calendar, next to the calendar click More, then Settings and sharing, then \
             Shared with, and add {own} with “{PERMISSION}”."
        ),
        String::new(),
        "Thank you.".to_string(),
    ]
    .join("\r\n");
    format!(
        "mailto:{}?subject={}&body={}",
        encoded(owner, true),
        encoded(&subject, false),
        encoded(&body, false)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar::CalendarFacts;

    /// A calendar on `provider` with `role`, its Google id `remote`, and the owner Google named.
    fn cal(provider: &str, role: Option<&str>, remote: &str, named: Option<&str>) -> Calendar {
        Calendar {
            id: format!("gcal:me@x.org:{remote}"),
            source_id: "gcal:me@x.org".into(),
            provider: provider.into(),
            remote_id: Some(remote.into()),
            name: "Team".into(),
            color: None,
            selected: true,
            is_primary: false,
            quiet: false,
            kind: None,
            facts: CalendarFacts {
                access_role: role.map(str::to_string),
                data_owner: named.map(str::to_string),
                ..CalendarFacts::default()
            },
        }
    }

    /// Plan Q10 and A12: only a view-only Google calendar, owned by a person PM can name.
    #[test]
    fn whom_to_ask_follows_the_role_and_the_owner() {
        let r = Some("reader");
        let rows: &[(&str, Calendar, Option<&str>)] = &[
            (
                "K1",
                cal(
                    "google",
                    r,
                    "team@group.calendar.google.com",
                    Some("boss@x.org"),
                ),
                Some("boss@x.org"),
            ),
            (
                "K2",
                cal("google", r, "colleague@x.org", None),
                Some("colleague@x.org"),
            ),
            (
                "K3",
                cal("google", Some("freeBusyReader"), "colleague@x.org", None),
                Some("colleague@x.org"),
            ),
            (
                "K4",
                cal("google", Some("writer"), "colleague@x.org", None),
                None,
            ),
            ("K5", cal("google", Some("owner"), "me@x.org", None), None),
            (
                "K6",
                cal(
                    "google",
                    Some("writerWithoutPrivateAccess"),
                    "colleague@x.org",
                    None,
                ),
                None,
            ),
            ("K7", cal("google", None, "colleague@x.org", None), None),
            (
                "K8",
                cal("google", Some("newRole"), "colleague@x.org", None),
                None,
            ),
            (
                "K9",
                cal(
                    "google",
                    r,
                    "en.usa#holiday@group.v.calendar.google.com",
                    None,
                ),
                None,
            ),
            (
                "K10",
                cal(
                    "google",
                    r,
                    "addressbook#contacts@group.v.calendar.google.com",
                    None,
                ),
                None,
            ),
            (
                "K11",
                cal("google", r, "abc@import.calendar.google.com", None),
                None,
            ),
            (
                "K12",
                cal("google", r, "room@resource.calendar.google.com", None),
                None,
            ),
            (
                "K13",
                cal("google", r, "team@group.calendar.google.com", None),
                None,
            ),
            (
                "K14",
                cal("google", r, "colleague@x.org", Some("not an address")),
                None,
            ),
            (
                "K15",
                cal(
                    "google",
                    r,
                    "team@group.calendar.google.com",
                    Some("x@GROUP.Calendar.google.com"),
                ),
                None,
            ),
            (
                "K16",
                cal("google", r, "colleague@x.org", Some("boss@x.org")),
                Some("boss@x.org"),
            ),
            (
                "K17",
                cal("google", r, "Colleague@X.org", None),
                Some("Colleague@X.org"),
            ),
            ("K18", cal("microsoft", r, "colleague@x.org", None), None),
            // The owner Google named is a calendar of its own (a group's id), not a person.
            (
                "K19",
                cal(
                    "google",
                    r,
                    "colleague@x.org",
                    Some("team@group.calendar.google.com"),
                ),
                None,
            ),
            // No id to fall back on.
            (
                "K20",
                Calendar {
                    remote_id: None,
                    ..cal("google", r, "x", None)
                },
                None,
            ),
        ];
        for (name, calendar, want) in rows {
            assert!(
                owner_to_ask(calendar).as_deref() == *want,
                "{name}: whom to ask"
            );
        }
    }

    // Messages name a row by its place, never by the address in it (CodeQL reads any address as
    // private, made up or not).
    #[test]
    fn plain_addresses_are_plain() {
        for (i, good) in [
            "a@b.co",
            "a.b+c@d-e.org",
            "o'neil@x.org",
            "a_b%c@x.y.org",
            &format!("{}@x.org", "a".repeat(64)),
        ]
        .iter()
        .enumerate()
        {
            assert!(is_plain_address(good), "plain row {i}");
        }
        let long = format!("a@{}.org", "b".repeat(248));
        assert_eq!(long.len(), 254);
        assert!(is_plain_address(&long), "254 characters is plain");
        for (i, bad) in [
            "a@b",
            "@b.co",
            "a@@b.co",
            "a b@c.co",
            "a@b..co",
            "a@-b.co",
            "a@b-.co",
            "a@b.co\r\n",
            " a@b.co",
            "a@b.co?cc=x",
            "a@b.co&body=x",
            "a#b@c.co",
            "a@b_c.co",
            "a@b.cö",
            "",
            &format!("{}@x.org", "a".repeat(65)),
            &format!("a@{}.org", "b".repeat(249)),
        ]
        .iter()
        .enumerate()
        {
            assert!(!is_plain_address(bad), "not-plain row {i}");
        }
    }

    #[test]
    fn calendar_domains_are_googles_own() {
        for (i, domain) in [
            "x@calendar.google.com",
            "x@group.calendar.google.com",
            "x@group.v.calendar.google.com",
            "x@IMPORT.Calendar.Google.com",
            "x@resource.calendar.google.com",
        ]
        .iter()
        .enumerate()
        {
            assert!(on_calendar_domain(domain), "Google's row {i}");
        }
        for (i, person) in [
            "x@google.com",
            "x@mycalendar.google.com",
            "x@calendar.google.com.evil.org",
        ]
        .iter()
        .enumerate()
        {
            assert!(!on_calendar_domain(person), "a person's row {i}");
        }
    }

    #[test]
    fn the_overview_says_who_is_connected() {
        let k1 = cal(
            "google",
            Some("reader"),
            "team@group.calendar.google.com",
            Some("boss@x.org"),
        );
        let k2 = cal("google", Some("reader"), "colleague@x.org", None);
        let k4 = cal("google", Some("writer"), "colleague@y.org", None);
        let map = ask_map(&[k1.clone(), k2.clone(), k4.clone()], &["BOSS@x.org"]);
        assert_eq!(map.len(), 2);
        assert!(
            map.get(&k1.id)
                == Some(&AskOwner {
                    owner: "boss@x.org".into(),
                    connected: true,
                }),
            "K1's owner is connected, whatever the case"
        );
        assert!(
            map.get(&k2.id)
                == Some(&AskOwner {
                    owner: "colleague@x.org".into(),
                    connected: false,
                }),
            "K2's owner isn't"
        );
        assert!(
            !map.contains_key(&k4.id),
            "an editable calendar asks nobody"
        );
        let json = serde_json::to_value(&map[&k1.id]).unwrap();
        assert!(
            json == serde_json::json!({"owner": "boss@x.org", "connected": true}),
            "the view reads both fields flat"
        );
    }

    /// The command's decision: whom to ask and the email, from PM's calendar id alone.
    #[test]
    fn asking_needs_a_calendar_an_owner_and_your_own_address() {
        let k1 = cal(
            "google",
            Some("reader"),
            "team@group.calendar.google.com",
            Some("boss@x.org"),
        );
        let calendars = [k1.clone()];
        let me = [("gcal:me@x.org", "me@x.org")];
        let (owner, link) = ask_plan(&calendars, &me, &k1.id).expect("K1 asks its owner");
        assert!(owner == "boss@x.org", "the owner Google named");
        assert!(
            link == mailto("boss@x.org", "me@x.org", "Team"),
            "the email"
        );
        assert!(
            ask_plan(&calendars, &me, "gcal:me@x.org:gone").is_none(),
            "an unknown id"
        );
        let both = [
            ("gcal:me@x.org", "me@x.org"),
            ("gcal:boss@x.org", "Boss@x.org"),
        ];
        assert!(
            ask_plan(&calendars, &both, &k1.id).is_none(),
            "a connected owner"
        );
        let other = [("gcal:you@x.org", "you@x.org")];
        assert!(
            ask_plan(&calendars, &other, &k1.id).is_none(),
            "no address for its own sign-in"
        );
        let blank = [("gcal:me@x.org", " ")];
        assert!(
            ask_plan(&calendars, &blank, &k1.id).is_none(),
            "a blank address"
        );
        let editable = [cal("google", Some("writer"), "colleague@x.org", None)];
        assert!(
            ask_plan(&editable, &me, &editable[0].id).is_none(),
            "editable"
        );
    }

    /// The test's own reading of a `mailto:` link: its address, subject and body, decoded.
    fn decode(link: &str) -> (String, String, String) {
        let unescape = |s: &str| {
            let bytes = s.as_bytes();
            let mut out = Vec::new();
            let mut i = 0;
            while i < bytes.len() {
                if bytes[i] == b'%' {
                    out.push(u8::from_str_radix(&s[i + 1..i + 3], 16).unwrap());
                    i += 3;
                } else {
                    out.push(bytes[i]);
                    i += 1;
                }
            }
            String::from_utf8(out).unwrap()
        };
        let rest = link.strip_prefix("mailto:").unwrap();
        let (to, query) = rest.split_once('?').unwrap();
        let (subject, body) = query.split_once('&').unwrap();
        (
            unescape(to),
            unescape(subject.strip_prefix("subject=").unwrap()),
            unescape(body.strip_prefix("body=").unwrap()),
        )
    }

    #[test]
    fn the_email_is_encoded_for_any_name() {
        let link = mailto("a+b@x.org", "me@y.org", "Q&A ? 100% #1 “Team”");
        let want = "mailto:a%2Bb@x.org?subject=Edit%20access%20to%20your%20calendar%20%E2%80%9CQ%26A%20%3F%20100%25%20%231%20%E2%80%9CTeam%E2%80%9D%E2%80%9D&body=Hi%2C%0D%0A%0D%0ACould%20you%20let%20me%40y.org%20make%20changes%20to%20your%20Google%20calendar%20%E2%80%9CQ%26A%20%3F%20100%25%20%231%20%E2%80%9CTeam%E2%80%9D%E2%80%9D%3F%0D%0A%0D%0AIn%20Google%20Calendar%2C%20next%20to%20the%20calendar%20click%20More%2C%20then%20Settings%20and%20sharing%2C%20then%20Shared%20with%2C%20and%20add%20me%40y.org%20with%20%E2%80%9CMake%20changes%20and%20see%20event%20details%E2%80%9D.%0D%0A%0D%0AThank%20you.";
        assert!(link == want, "the email's link, byte for byte");
        assert_eq!(link.len(), 583);
        assert_eq!(link.matches('?').count(), 1);
        assert_eq!(link.matches('&').count(), 1);
        assert!(!link.contains([' ', '\r', '\n', '+']));
        let (to, subject, body) = decode(&link);
        assert!(to == "a+b@x.org", "the address decodes");
        assert!(
            subject == "Edit access to your calendar “Q&A ? 100% #1 “Team””",
            "the subject decodes"
        );
        assert!(
            body
                == "Hi,\r\n\r\nCould you let me@y.org make changes to your Google calendar “Q&A ? \
                     100% #1 “Team””?\r\n\r\nIn Google Calendar, next to the calendar click More, \
                     then Settings and sharing, then Shared with, and add me@y.org with “Make \
                     changes and see event details”.\r\n\r\nThank you.",
            "the body decodes, line by line"
        );
        // RFC 6068: only the unreserved characters stay as they are, and `@` only in the address.
        assert!(
            encoded("a-b_c.d~e9", false) == "a-b_c.d~e9",
            "the unreserved"
        );
        let named = mailto("a@x.org", "m", "b@c");
        assert!(named.starts_with("mailto:a@x.org?subject="), "the address");
        assert!(
            !named.contains("b@c") && named.matches("b%40c").count() == 2,
            "a name's @ is encoded in the subject and the body"
        );
    }

    /// Whoever made the calendar named it: the name can't add a header or a line to the email.
    #[test]
    fn a_name_stays_on_one_line() {
        let link = mailto("a@x.org", "me@y.org", "Team\r\nBcc: z@x.org\tx");
        let (_, subject, body) = decode(&link);
        assert!(
            subject == "Edit access to your calendar “Team  Bcc: z@x.org x”",
            "the name's line break reads as spaces in the subject"
        );
        assert!(
            body.matches("\r\n").count() == 6,
            "the body has only its own line breaks"
        );
        // A name cut at the most: the break is past the cut, or it reads as a space before it.
        let late = format!("{}\nmore", "a".repeat(NAME_MAX));
        let (_, subject, _) = decode(&mailto("a@x.org", "me@y.org", &late));
        assert!(
            subject == format!("Edit access to your calendar “{}…”", "a".repeat(NAME_MAX)),
            "the cut comes after the name's own characters"
        );
        let early = format!("a\n{}", "a".repeat(NAME_MAX));
        let (_, subject, _) = decode(&mailto("a@x.org", "me@y.org", &early));
        assert!(
            !subject.contains('\n') && subject.contains("“a a"),
            "a line break before the cut reads as a space too"
        );
    }

    #[test]
    fn a_long_name_is_cut_and_the_link_stays_short() {
        let name = "界".repeat(NAME_MAX + 1);
        let owner = format!("{}@x.org", "o".repeat(94));
        let own = format!("{}@y.org", "m".repeat(94));
        assert_eq!((owner.len(), own.len()), (100, 100));
        let link = mailto(&owner, &own, &name);
        let (_, subject, body) = decode(&link);
        let cut = format!("“{}…”", "界".repeat(NAME_MAX));
        assert!(
            subject == format!("Edit access to your calendar {cut}"),
            "the subject's name is cut"
        );
        assert!(
            body.contains(&format!("calendar {cut}?")),
            "the body's name is cut too"
        );
        assert!(link.len() <= 2000, "the link stays short");
        assert_eq!(link.len(), 1490);
        // A name of exactly the most is whole.
        let whole = "界".repeat(NAME_MAX);
        let (_, subject, _) = decode(&mailto(&owner, &own, &whole));
        assert!(
            subject == format!("Edit access to your calendar “{whole}”"),
            "a name of the most is whole"
        );
    }
}
