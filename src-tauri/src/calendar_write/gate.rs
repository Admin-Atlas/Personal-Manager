// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What PM may change about an event, and why not. One pure function, [`edit_rights`], answers it
//! from the event's facts. The list runs it on the mirror's copy to decide whether to offer Edit; every
//! save runs it again on a fresh copy from Google before sending anything. It fails closed: a role or
//! an event type PM doesn't know reads as read-only, never as editable.
//!
//! Events PM can change are the ones you organise on a calendar you can edit. The first layers
//! edited only solo, one-off events; repeating events (C9) and events with guests (C12) are now
//! editable, each with the choice it stood in for (rule R9): a change to an occurrence names which
//! occurrences it's for ("this event", "this and following", "all events"), a change Google would
//! email guests about names whether it should, and a write without that choice is asked again.
//! Invitations stay read-only until C14. A one-off that Google has since turned into a repeating
//! event is read-only with its own reason on every path, since a write to it would change every
//! occurrence with no choice asked.

use crate::calendar_editing::EditingStatus;

use super::dto::{FieldPermissions, ReadOnlyReason};

/// Everything the gate needs to know about one event.
#[derive(Debug, Clone, Copy)]
pub struct EditFacts<'a> {
    /// The event comes from a Google account PM signs in to (`gcal:`), not a feed or Outlook.
    pub google: bool,
    pub editing: EditingStatus,
    /// The calendar's `accessRole`.
    pub access_role: Option<&'a str>,
    /// Google's `eventType`; absent means `default`, as Google documents.
    pub event_type: Option<&'a str>,
    /// Google's `organizer.self`: the organiser is the calendar this copy is on.
    pub organizer_self: bool,
    pub locked: bool,
    pub visibility: Option<&'a str>,
    /// Google's copy is a repeating event's series itself (its rule, no `recurringEventId`), while PM
    /// asked about one event: a one-off that had a rule added in Google since. Saving or deleting it
    /// would change every occurrence with no choice asked (C9 design §0.7, critique 2 M6). Known only
    /// from a fresh copy; the mirror never holds a series itself.
    pub series_master: bool,
    /// The description is formatted. Known only from a fresh copy (the mirror clips descriptions).
    pub html_description: bool,
}

/// What may change about the event described by `facts`.
pub fn edit_rights(facts: &EditFacts) -> FieldPermissions {
    if let Some(reason) = blanket_reason(facts) {
        return FieldPermissions::none(reason);
    }
    let mut perms = FieldPermissions {
        summary: true,
        time: true,
        location: true,
        description: true,
        show_as: true,
        visibility: true,
        guests: true,
        delete: true,
        reasons: Vec::new(),
    };
    if facts.locked {
        // Google refuses changes to a locked copy's summary, description, location, start, end and
        // recurrence. Whether it lets the copy be deleted, or its guests change, isn't documented,
        // so PM offers neither (INFERRED: fails closed, C12 §4.4).
        perms.summary = false;
        perms.time = false;
        perms.location = false;
        perms.description = false;
        perms.guests = false;
        perms.delete = false;
        perms.reasons.push(ReadOnlyReason::Locked);
    }
    if facts.html_description && perms.description {
        perms.description = false;
        perms.reasons.push(ReadOnlyReason::HtmlDescription);
    }
    perms
}

/// The one reason the list shows when nothing about the event may change, or `None` when something
/// can. Computed from the mirror's copy, so a formatted description (unknown there) never blocks it.
pub fn edit_block(facts: &EditFacts) -> Option<ReadOnlyReason> {
    let perms = edit_rights(facts);
    if perms.any() {
        None
    } else {
        perms.reasons.first().copied()
    }
}

/// Why PM may not create an event with `visibility` on a calendar, or `None` when it may: the account
/// and calendar rules of [`edit_rights`], for a new event you organise, with guests or none (C12d).
/// Q1: a `writerWithoutPrivateAccess` calendar takes every event but a private one. `google` holds
/// already: the caller resolved the calendar, and a feed or an Outlook calendar never gets here.
pub fn create_block(
    editing: EditingStatus,
    access_role: Option<&str>,
    visibility: Option<&str>,
) -> Option<ReadOnlyReason> {
    blanket_reason(&EditFacts {
        google: true,
        editing,
        access_role,
        event_type: None,
        organizer_self: true,
        locked: false,
        visibility,
        series_master: false,
        html_description: false,
    })
}

/// The reason nothing at all may change, checked account first, then calendar, then the event
/// itself; the first one found is the one shown.
fn blanket_reason(facts: &EditFacts) -> Option<ReadOnlyReason> {
    if !facts.google {
        return Some(ReadOnlyReason::NotGoogle);
    }
    if facts.editing != EditingStatus::On {
        return Some(ReadOnlyReason::EditingOff);
    }
    match facts.access_role {
        Some("owner" | "writer") => {}
        // Q1 (2026-10-09): can edit everything but private events.
        Some("writerWithoutPrivateAccess") => {
            if matches!(facts.visibility, Some("private" | "confidential")) {
                return Some(ReadOnlyReason::PrivateEvent);
            }
        }
        // reader, freeBusyReader, missing, or a role Google adds later.
        _ => return Some(ReadOnlyReason::CalendarReadOnly),
    }
    match facts.event_type {
        None | Some("default") => {}
        // birthday, fromGmail, focusTime, outOfOffice, workingLocation, or a type Google adds later.
        Some(_) => return Some(ReadOnlyReason::SpecialType),
    }
    if !facts.organizer_self {
        return Some(ReadOnlyReason::NotOrganizer);
    }
    if facts.series_master {
        return Some(ReadOnlyReason::BecameRepeating);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use ReadOnlyReason::*;

    /// A solo, one-off event you organise on your own calendar, with editing on.
    fn editable() -> EditFacts<'static> {
        EditFacts {
            google: true,
            editing: EditingStatus::On,
            access_role: Some("owner"),
            event_type: Some("default"),
            organizer_self: true,
            locked: false,
            visibility: None,
            series_master: false,
            html_description: false,
        }
    }

    #[test]
    fn a_solo_event_you_organise_is_fully_editable() {
        let perms = edit_rights(&editable());
        assert!(perms.summary && perms.time && perms.location && perms.description);
        assert!(perms.show_as && perms.visibility && perms.delete);
        // Guests may change wherever the rest may (C12).
        assert!(perms.guests);
        assert!(perms.reasons.is_empty());
        assert_eq!(edit_block(&editable()), None);
        // Absent eventType is Google's documented default.
        let no_type = EditFacts {
            event_type: None,
            ..editable()
        };
        assert_eq!(edit_block(&no_type), None);
    }

    /// Each blanket reason, in the order the user can do least about it.
    #[test]
    fn every_blanket_reason_blocks_everything() {
        let e = editable;
        let cases: Vec<(EditFacts, ReadOnlyReason)> = vec![
            (
                EditFacts {
                    google: false,
                    ..e()
                },
                NotGoogle,
            ),
            (
                EditFacts {
                    editing: EditingStatus::Off,
                    ..e()
                },
                EditingOff,
            ),
            (
                EditFacts {
                    editing: EditingStatus::Paused,
                    ..e()
                },
                EditingOff,
            ),
            (
                EditFacts {
                    editing: EditingStatus::NeedsConsent,
                    ..e()
                },
                EditingOff,
            ),
            (
                EditFacts {
                    access_role: Some("reader"),
                    ..e()
                },
                CalendarReadOnly,
            ),
            (
                EditFacts {
                    access_role: Some("freeBusyReader"),
                    ..e()
                },
                CalendarReadOnly,
            ),
            (
                EditFacts {
                    access_role: None,
                    ..e()
                },
                CalendarReadOnly,
            ),
            // A role Google adds later is read-only until PM knows what it allows.
            (
                EditFacts {
                    access_role: Some("superEditor"),
                    ..e()
                },
                CalendarReadOnly,
            ),
            (
                EditFacts {
                    access_role: Some("writerWithoutPrivateAccess"),
                    visibility: Some("private"),
                    ..e()
                },
                PrivateEvent,
            ),
            (
                EditFacts {
                    access_role: Some("writerWithoutPrivateAccess"),
                    visibility: Some("confidential"),
                    ..e()
                },
                PrivateEvent,
            ),
            (
                EditFacts {
                    event_type: Some("birthday"),
                    ..e()
                },
                SpecialType,
            ),
            (
                EditFacts {
                    event_type: Some("fromGmail"),
                    ..e()
                },
                SpecialType,
            ),
            (
                EditFacts {
                    event_type: Some("focusTime"),
                    ..e()
                },
                SpecialType,
            ),
            (
                EditFacts {
                    event_type: Some("outOfOffice"),
                    ..e()
                },
                SpecialType,
            ),
            (
                EditFacts {
                    event_type: Some("workingLocation"),
                    ..e()
                },
                SpecialType,
            ),
            (
                EditFacts {
                    event_type: Some("somethingNew"),
                    ..e()
                },
                SpecialType,
            ),
            (
                EditFacts {
                    organizer_self: false,
                    ..e()
                },
                NotOrganizer,
            ),
            (
                EditFacts {
                    series_master: true,
                    ..e()
                },
                BecameRepeating,
            ),
            // A read-only shared calendar outranks "it became a repeating event".
            (
                EditFacts {
                    access_role: Some("reader"),
                    series_master: true,
                    ..e()
                },
                CalendarReadOnly,
            ),
        ];
        for (facts, want) in cases {
            let perms = edit_rights(&facts);
            assert!(!perms.any(), "{facts:?}");
            assert_eq!(perms.reasons, vec![want], "{facts:?}");
            assert_eq!(edit_block(&facts), Some(want), "{facts:?}");
        }
    }

    /// Q1: a writer-without-private-access may edit everything but private events.
    #[test]
    fn a_writer_without_private_access_edits_non_private_events() {
        for visibility in [None, Some("default"), Some("public")] {
            let facts = EditFacts {
                access_role: Some("writerWithoutPrivateAccess"),
                visibility,
                ..editable()
            };
            assert_eq!(edit_block(&facts), None, "{visibility:?}");
        }
    }

    /// Who may create where (C10): the account's editing first, then the calendar's role, with Q1's
    /// one exception for a private event on a calendar that can't hold one.
    #[test]
    fn create_block_follows_q1() {
        let on = EditingStatus::On;
        let cases = [
            (on, Some("owner"), None, None),
            (on, Some("owner"), Some("private"), None),
            (on, Some("writer"), Some("confidential"), None),
            (on, Some("writerWithoutPrivateAccess"), None, None),
            (
                on,
                Some("writerWithoutPrivateAccess"),
                Some("default"),
                None,
            ),
            (on, Some("writerWithoutPrivateAccess"), Some("public"), None),
            (
                on,
                Some("writerWithoutPrivateAccess"),
                Some("private"),
                Some(PrivateEvent),
            ),
            (
                on,
                Some("writerWithoutPrivateAccess"),
                Some("confidential"),
                Some(PrivateEvent),
            ),
            (on, Some("reader"), None, Some(CalendarReadOnly)),
            (on, Some("freeBusyReader"), None, Some(CalendarReadOnly)),
            (on, None, None, Some(CalendarReadOnly)),
            (on, Some("someNewRole"), None, Some(CalendarReadOnly)),
            // Editing comes before the role: an account that can't write says so first.
            (EditingStatus::Paused, Some("owner"), None, Some(EditingOff)),
            (
                EditingStatus::NeedsConsent,
                Some("owner"),
                None,
                Some(EditingOff),
            ),
            (EditingStatus::Off, Some("reader"), None, Some(EditingOff)),
        ];
        for (editing, role, visibility, want) in cases {
            assert_eq!(
                create_block(editing, role, visibility),
                want,
                "{editing:?} {role:?} {visibility:?}"
            );
        }
    }

    #[test]
    fn a_locked_copy_keeps_only_what_google_lets_it_change() {
        let perms = edit_rights(&EditFacts {
            locked: true,
            ..editable()
        });
        assert!(!perms.summary && !perms.time && !perms.location && !perms.description);
        assert!(!perms.delete);
        // Not among Google's locked fields, but undocumented, so closed (C12 §4.4).
        assert!(!perms.guests);
        assert!(perms.show_as && perms.visibility);
        assert_eq!(perms.reasons, vec![Locked]);
        // Something can still change, so the list offers Edit.
        assert_eq!(
            edit_block(&EditFacts {
                locked: true,
                ..editable()
            }),
            None
        );
    }

    #[test]
    fn a_formatted_description_is_read_only_and_says_why() {
        let perms = edit_rights(&EditFacts {
            html_description: true,
            ..editable()
        });
        assert!(!perms.description);
        assert!(perms.summary && perms.time && perms.delete);
        assert_eq!(perms.reasons, vec![HtmlDescription]);
        // Locked already covers the description, so it isn't given twice.
        let both = edit_rights(&EditFacts {
            html_description: true,
            locked: true,
            ..editable()
        });
        assert_eq!(both.reasons, vec![Locked]);
    }
}
