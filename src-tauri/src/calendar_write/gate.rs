// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What PM may change about an event, and why not. One pure function, [`edit_rights`], answers it
//! from the event's facts. The list runs it on the mirror's copy to decide whether to offer Edit; every
//! save runs it again on a fresh copy from Google before sending anything. It fails closed: a role or
//! an event type PM doesn't know reads as read-only, never as editable.
//!
//! The first layers edit only solo, one-off events you organise. Repeating events, events with
//! guests and invitations are read-only with a reason until the layers that handle them (C9, C12,
//! C14) lift that reason, each replacing it with the rule it stood in for (rule R9).

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
    pub recurring: bool,
    /// Guests besides the calendar itself.
    pub has_guests: bool,
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
        delete: true,
        reasons: Vec::new(),
    };
    if facts.locked {
        // Google refuses changes to a locked copy's summary, description, location, start, end and
        // recurrence. Whether it lets the copy be deleted isn't documented, so PM doesn't offer it.
        perms.summary = false;
        perms.time = false;
        perms.location = false;
        perms.description = false;
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
    if facts.recurring {
        return Some(ReadOnlyReason::Recurring);
    }
    if facts.has_guests {
        return Some(ReadOnlyReason::HasGuests);
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
            recurring: false,
            has_guests: false,
            html_description: false,
        }
    }

    #[test]
    fn a_solo_event_you_organise_is_fully_editable() {
        let perms = edit_rights(&editable());
        assert!(perms.summary && perms.time && perms.location && perms.description);
        assert!(perms.show_as && perms.visibility && perms.delete);
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
                    recurring: true,
                    ..e()
                },
                Recurring,
            ),
            (
                EditFacts {
                    has_guests: true,
                    ..e()
                },
                HasGuests,
            ),
            // The first reason wins: a read-only shared calendar outranks "it repeats".
            (
                EditFacts {
                    access_role: Some("reader"),
                    recurring: true,
                    has_guests: true,
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

    #[test]
    fn a_locked_copy_keeps_only_what_google_lets_it_change() {
        let perms = edit_rights(&EditFacts {
            locked: true,
            ..editable()
        });
        assert!(!perms.summary && !perms.time && !perms.location && !perms.description);
        assert!(!perms.delete);
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
