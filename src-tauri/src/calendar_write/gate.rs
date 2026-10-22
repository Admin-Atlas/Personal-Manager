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
//! Invitations stay read-only in the editor until C14's allow-list; replying is apart from that
//! ([`reply_block`]): it follows whether the calendar is on the guest list, not who organises the
//! event. A one-off that Google has since turned into a repeating event is read-only with its own
//! reason on every path, since a write to it would change every occurrence with no choice asked.

use crate::calendar_editing::EditingStatus;

use super::dto::{FieldPermissions, ListRights, ReadOnlyReason};

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
    /// The event has a Google Calendar label (`eventLabelId`), which sets its colour (C13).
    pub labelled: bool,
    /// PM can read every notification Google holds on it ([`super::patch::reminders_of`]). Known
    /// only from a fresh copy: the mirror doesn't hold notifications, so the list reads true.
    pub reminders_readable: bool,
    /// The calendar this copy is on has an entry on the guest list (Google's `self`): it has a reply
    /// to give, the organiser's own included (C14).
    pub invited: bool,
}

/// What may change about the event described by `facts`, and whether the user may reply.
pub fn edit_rights(facts: &EditFacts) -> FieldPermissions {
    let respond = reply_block(facts).is_none();
    if let Some(reason) = blanket_reason(facts) {
        return FieldPermissions {
            respond,
            ..FieldPermissions::none(reason)
        };
    }
    let mut perms = FieldPermissions {
        summary: true,
        time: true,
        location: true,
        description: true,
        show_as: true,
        visibility: true,
        guests: true,
        reminders: true,
        colour: true,
        delete: true,
        // A lock is Google's on the shared fields: a reply isn't among them.
        respond,
        reasons: Vec::new(),
    };
    if facts.locked {
        // Google refuses changes to a locked copy's summary, description, location, start, end and
        // recurrence. Whether it lets the copy be deleted, or its guests change, isn't documented,
        // so PM offers neither (INFERRED: fails closed, C12 §4.4). Its notifications and colour
        // aren't among them, so they stay open (the editing plan's C6 card, from C13).
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
    // A label sets the colour Google draws, and PM doesn't change labels (R-LABEL).
    if facts.labelled {
        perms.colour = false;
        perms.reasons.push(ReadOnlyReason::LabelReadOnly);
    }
    // A list sent whole would lose the one PM can't read.
    if !facts.reminders_readable {
        perms.reminders = false;
        perms.reasons.push(ReadOnlyReason::RemindersUnreadable);
    }
    perms
}

/// What the list offers on a row (see [`ListRights`]): the one reason it shows when nothing in the
/// editor may change (`None` when something can), Delete exactly where a delete's own check would
/// allow it, and the reply. Computed from the mirror's copy, so a formatted description (unknown
/// there) never blocks it.
pub fn list_rights(facts: &EditFacts) -> ListRights {
    let perms = edit_rights(facts);
    ListRights {
        edit_block: if perms.any() {
            None
        } else {
            perms.reasons.first().copied()
        },
        can_delete: perms.delete,
        can_respond: perms.respond,
    }
}

/// Why the user can't reply from this copy, or `None` when they can: the reasons nothing at all may
/// be written from it, then an entry on the guest list. The organiser has one too, and may reply; a
/// lock doesn't stop it (Google locks the shared fields), and neither does a one-off Google has
/// since made repeating, which the reply's own read refuses as a save does.
pub fn reply_block(facts: &EditFacts) -> Option<ReadOnlyReason> {
    access_reason(facts).or((!facts.invited).then_some(ReadOnlyReason::NotInvited))
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
        labelled: false,
        reminders_readable: true,
        invited: false,
    })
}

/// The reason nothing at all may change, checked account first, then calendar, then the event
/// itself; the first one found is the one shown.
fn blanket_reason(facts: &EditFacts) -> Option<ReadOnlyReason> {
    if let Some(reason) = access_reason(facts) {
        return Some(reason);
    }
    if !facts.organizer_self {
        return Some(ReadOnlyReason::NotOrganizer);
    }
    if facts.series_master {
        return Some(ReadOnlyReason::BecameRepeating);
    }
    None
}

/// Account, calendar and kind: the reasons nothing at all may be written from this copy, a reply
/// included. Checked in this order; the first found is the one shown.
fn access_reason(facts: &EditFacts) -> Option<ReadOnlyReason> {
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
            labelled: false,
            reminders_readable: true,
            invited: false,
        }
    }

    /// The list's one reason, as [`list_rights`] gives it.
    fn edit_block(facts: &EditFacts) -> Option<ReadOnlyReason> {
        list_rights(facts).edit_block
    }

    /// An invitation on your own calendar: someone else organises it, and your calendar is on its
    /// guest list.
    fn invitation() -> EditFacts<'static> {
        EditFacts {
            organizer_self: false,
            invited: true,
            ..editable()
        }
    }

    #[test]
    fn a_solo_event_you_organise_is_fully_editable() {
        let perms = edit_rights(&editable());
        assert!(perms.summary && perms.time && perms.location && perms.description);
        assert!(perms.show_as && perms.visibility && perms.delete);
        // Guests may change wherever the rest may (C12), and so may notifications and colour (C13).
        assert!(perms.guests);
        assert!(perms.reminders && perms.colour);
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
        // C13: notifications and colour aren't among Google's locked fields.
        assert!(perms.reminders && perms.colour);
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

    /// R-LABEL (C13): a label sets the colour Google draws, so a labelled event's colour is
    /// read-only and says why; its notifications and everything else stay open.
    #[test]
    fn a_labelled_events_colour_is_read_only_and_says_why() {
        let perms = edit_rights(&EditFacts {
            labelled: true,
            ..editable()
        });
        assert!(!perms.colour);
        assert!(perms.reminders && perms.summary && perms.time && perms.delete);
        assert_eq!(perms.reasons, vec![LabelReadOnly]);
        // Locked as well: both reasons, the lock's first.
        let both = edit_rights(&EditFacts {
            labelled: true,
            locked: true,
            ..editable()
        });
        assert!(!both.colour && both.reminders);
        assert_eq!(both.reasons, vec![Locked, LabelReadOnly]);
        // Something can still change, so the list offers Edit.
        assert_eq!(
            edit_block(&EditFacts {
                labelled: true,
                ..editable()
            }),
            None
        );
    }

    /// C13: a list PM can't read whole is read-only, with its reason; the colour stays open.
    #[test]
    fn unreadable_notifications_are_read_only_with_their_reason() {
        let perms = edit_rights(&EditFacts {
            reminders_readable: false,
            ..editable()
        });
        assert!(!perms.reminders);
        assert!(perms.colour && perms.summary);
        assert_eq!(perms.reasons, vec![RemindersUnreadable]);
    }

    /// A blanket reason closes notifications and colour too: nothing at all may change.
    #[test]
    fn a_blanket_reason_closes_notifications_and_colour() {
        let perms = edit_rights(&EditFacts {
            organizer_self: false,
            labelled: true,
            reminders_readable: false,
            ..editable()
        });
        assert!(!perms.reminders && !perms.colour && !perms.any());
        assert_eq!(perms.reasons, vec![NotOrganizer]);
    }

    /// C14: a reply follows the guest list, not who organises the event: the reasons nothing may be
    /// written from the copy, then an entry of its own on the list. A lock doesn't stop it, nor does a
    /// one-off made repeating (the reply's own read refuses that, as a save does).
    #[test]
    fn a_reply_needs_an_entry_on_the_guest_list_not_the_organisers_role() {
        let i = invitation;
        let rows: Vec<(&str, EditFacts, bool, Option<ReadOnlyReason>)> = vec![
            ("R1", i(), true, None),
            (
                "R2",
                EditFacts {
                    organizer_self: true,
                    ..i()
                },
                true,
                None,
            ),
            (
                "R3",
                EditFacts {
                    invited: false,
                    ..i()
                },
                false,
                Some(NotInvited),
            ),
            (
                "R4",
                EditFacts {
                    event_type: Some("birthday"),
                    ..i()
                },
                false,
                Some(SpecialType),
            ),
            (
                "R5",
                EditFacts {
                    editing: EditingStatus::Off,
                    ..i()
                },
                false,
                Some(EditingOff),
            ),
            (
                "R6",
                EditFacts {
                    access_role: Some("reader"),
                    ..i()
                },
                false,
                Some(CalendarReadOnly),
            ),
            (
                "R7",
                EditFacts {
                    access_role: Some("writerWithoutPrivateAccess"),
                    visibility: Some("private"),
                    ..i()
                },
                false,
                Some(PrivateEvent),
            ),
            (
                "R8",
                EditFacts {
                    locked: true,
                    ..i()
                },
                true,
                None,
            ),
            (
                "R9",
                EditFacts {
                    google: false,
                    ..i()
                },
                false,
                Some(NotGoogle),
            ),
            (
                "R10",
                EditFacts {
                    organizer_self: true,
                    series_master: true,
                    ..i()
                },
                true,
                None,
            ),
            // The account's reasons come before the guest list's.
            (
                "R11",
                EditFacts {
                    editing: EditingStatus::Paused,
                    invited: false,
                    ..i()
                },
                false,
                Some(EditingOff),
            ),
        ];
        for (row, facts, respond, block) in rows {
            assert_eq!(edit_rights(&facts).respond, respond, "{row}");
            assert_eq!(reply_block(&facts), block, "{row}");
            assert_eq!(list_rights(&facts).can_respond, respond, "{row}");
        }
    }

    /// C14: on an invitation the reply is open while nothing in the editor is: the list still says
    /// why the editor can't change it, offers no Delete, and offers the reply apart.
    #[test]
    fn a_reply_alone_is_not_an_edit() {
        let perms = edit_rights(&invitation());
        assert!(!perms.any() && perms.respond && !perms.delete);
        assert_eq!(perms.reasons, vec![NotOrganizer]);
        assert!(
            list_rights(&invitation())
                == ListRights {
                    edit_block: Some(NotOrganizer),
                    can_delete: false,
                    can_respond: true,
                },
            "an invitation: no editor, no Delete, the reply"
        );
        // Your own event: Delete as the gate allows it, and a reply only with an entry of your own.
        assert!(
            list_rights(&editable())
                == ListRights {
                    edit_block: None,
                    can_delete: true,
                    can_respond: false,
                }
        );
        let locked = EditFacts {
            locked: true,
            invited: true,
            ..editable()
        };
        assert!(
            list_rights(&locked)
                == ListRights {
                    edit_block: None,
                    can_delete: false,
                    can_respond: true,
                },
            "a lock keeps the delete closed and the reply open"
        );
    }
}
