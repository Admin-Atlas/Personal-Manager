// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What PM may change about an event, and why not. One pure function, [`edit_rights`], answers it
//! from the event's facts. The list runs it on the mirror's copy to decide whether to offer Edit; every
//! save runs it again on a fresh copy from Google before sending anything. It fails closed: a role or
//! an event type PM doesn't know reads as read-only, never as editable.
//!
//! Events PM can change are on a calendar you can edit. The first layers edited only solo, one-off
//! events you organise; repeating events (C9), events with guests (C12) and invitations (C14) are now
//! editable, each with the rule it stood in for (R9): a change to an occurrence names which
//! occurrences it's for ("this event", "this and following", "all events"), a change Google would
//! email guests about names whether it should, and a write without that choice is asked again. On an
//! invitation, the user's own notifications, colour and busy or free, their reply, and the shared
//! fields only when the organiser lets guests change them; never how it repeats or who is invited.
//! Its delete only takes the user's own copy off their calendar ([`removes_copy`]). Replying is apart
//! from the rest ([`reply_block`]): it follows whether the calendar is on the guest list, not who
//! organises the event. A one-off that Google has since turned into a repeating event is read-only
//! with its own reason on every path, since a write to it would change every occurrence with no
//! choice asked.
//!
//! A video call is Meet only, where the calendar offers it, and the organiser's to add or remove;
//! another service's call is shown and kept (C18). The calendar is the organiser's to change too, to
//! another of the same account; a locked event keeps its own (C18d).
//!
//! A description with formatting PM can't write back is read-only (C19, R9: the lift of the old
//! "formatted" reason); a save the user chose to make plain reopens it
//! ([`FieldPermissions::replacing_formatting`]).
//!
//! Birthdays and events Gmail added (C20) change only what Google's per-type list allows, every
//! other field starting closed (R9: the lift of their "special kind" reason); other special kinds
//! stay read-only.

use serde_json::Value;

use crate::calendar_editing::EditingStatus;

use super::dto::{CallKind, FieldPermissions, ListRights, Notify, ReadOnlyReason, TimeDraft};

/// Which of Google's event types PM edits, and by which rule. Read from `eventType`, where absent
/// means `default` (Google: "This cannot be modified after the event is created"). Named for what
/// Google lets change rather than for a birthday: CodeQL reads any name like that as private data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// An ordinary event: every rule before C20.
    Default,
    /// Google's `birthday` type: a birthday, an anniversary or another special date, all day, every
    /// year. Its name, notifications and colour may change, and its date when nothing else owns it.
    SpecialDate,
    /// Google's `fromGmail` type: Gmail added it. Busy or free, visibility, notifications, colour
    /// and the user's reply may change.
    FromGmail,
    /// Focus time, out of office, working location, or a type Google adds later: read-only
    /// (`SpecialType`) until their own card (Q8).
    Other,
}

/// `eventType` as a [`Kind`]. Unknown types fail closed (`Other`).
pub fn kind_of(event_type: Option<&str>) -> Kind {
    match event_type {
        None | Some("default") => Kind::Default,
        Some("birthday") => Kind::SpecialDate,
        Some("fromGmail") => Kind::FromGmail,
        Some(_) => Kind::Other,
    }
}

/// Whether a delete of this copy only takes it off the user's calendar ("Remove", C14d): an ordinary
/// event someone else organises. Google web: "After you remove an event, Calendar tells the event
/// organizer you decline the event". Never a birthday or a Gmail event, whose organiser Google fixes
/// and whose delete emails nobody (C20 §0.4 item 11). One rule for the send, the repeating event's
/// planner and the webview's `removesCopy`.
pub fn removes_copy(event_type: Option<&str>, organizer_self: bool) -> bool {
    kind_of(event_type) == Kind::Default && !organizer_self
}

/// What a removal tells Google to send: the organiser hears of the decline, and nothing is asked (Q6:
/// "a reply only ever reaches the organiser"). What Google does with it on a guest's copy is live test
/// L-I4's, and this line is what changes if it says otherwise.
pub const REMOVAL_NOTIFY: Notify = Notify::All;

/// Who owns a special date's day (Google's `birthdayProperties`): only one added in Google Calendar
/// may move. Google: "Timing details of a birthday event cannot be updated if the event is linked to
/// a contact, or its type is 'self'."
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateSource {
    /// Added in Google Calendar: no contact, and the type absent or Google's default.
    Calendar,
    /// Linked to a contact (an anniversary, a custom or an other date always is): Google Contacts
    /// owns it.
    Contacts,
    /// The calendar owner's own (`self`): their Google Account owns it.
    Profile,
    /// Not known: the mirror doesn't keep it, Google sent none, or a type PM doesn't know. The date
    /// stays.
    Unknown,
}

/// The date source of a fresh copy, read from `birthdayProperties` as Google documents it (its
/// events reference). Meaningful only for [`Kind::SpecialDate`]; pure. Anything PM can't read for
/// sure is `Unknown`, whose date stays: Google's guide says to "inspect the birthdayProperties
/// field" on every birthday it returns, so one without it isn't guessed to be the user's own (live
/// probe G-B0).
pub fn date_source_of(event: &Value) -> DateSource {
    let Some(props) = event.get("birthdayProperties").and_then(Value::as_object) else {
        return DateSource::Unknown;
    };
    let kind = match props.get("type") {
        None => None,
        Some(Value::String(kind)) => Some(kind.as_str()),
        Some(_) => return DateSource::Unknown,
    };
    if kind == Some("self") {
        return DateSource::Profile;
    }
    match props.get("contact") {
        None | Some(Value::Null) => {}
        Some(Value::String(contact)) if !contact.is_empty() => return DateSource::Contacts,
        Some(Value::String(_)) => {}
        // A contact PM can't read is no proof there is none (C20 critique C1-L7).
        Some(_) => return DateSource::Unknown,
    }
    match kind {
        // "Always has a contact" (events reference), whether or not Google sent one.
        Some("anniversary" | "custom" | "other") => DateSource::Contacts,
        None | Some("birthday") => DateSource::Calendar,
        Some(_) => DateSource::Unknown,
    }
}

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
    /// Google's `guestsCanModify` (absent = false): the organiser lets guests change the shared
    /// fields (C14).
    pub guests_can_modify: bool,
    pub locked: bool,
    pub visibility: Option<&'a str>,
    /// Google's copy is a repeating event's series itself (its rule, no `recurringEventId`), while PM
    /// asked about one event: a one-off that had a rule added in Google since. Saving or deleting it
    /// would change every occurrence with no choice asked (C9 design §0.7, critique 2 M6). Known only
    /// from a fresh copy; the mirror never holds a series itself.
    pub series_master: bool,
    /// The description has formatting PM's editor can't write back
    /// ([`super::description::pm_can_edit`]). Known only from a fresh copy (the mirror clips
    /// descriptions).
    pub unsupported_formatting: bool,
    /// The event has a Google Calendar label (`eventLabelId`), which sets its colour (C13).
    pub labelled: bool,
    /// PM can read every notification Google holds on it ([`super::patch::reminders_of`]). Known
    /// only from a fresh copy: the mirror doesn't hold notifications, so the list reads true.
    pub reminders_readable: bool,
    /// The calendar this copy is on has an entry on the guest list (Google's `self`): it has a reply
    /// to give, the organiser's own included (C14).
    pub invited: bool,
    /// The event's video call, by kind (`None`: none). From a fresh copy (`call::call_of`); the list
    /// reads the mirror's link (`call::kind_of_link`) (C18).
    pub call: Option<CallKind>,
    /// The calendar offers Google Meet (its `allowedConferenceSolutionTypes` has `hangoutsMeet`).
    pub meet_offered: bool,
    /// Who owns a special date's day ([`date_source_of`] on a fresh copy); `Unknown` on the list
    /// path and for every other kind.
    pub date_source: DateSource,
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
    let kind = kind_of(facts.event_type);
    let start = match kind {
        Kind::SpecialDate | Kind::FromGmail => special_rights(facts, kind),
        // `Other` never gets here (`access_reason` gave `SpecialType`); the arm keeps it total.
        Kind::Default | Kind::Other => default_rights(facts),
    };
    FieldPermissions {
        respond,
        ..closing_rules(start, facts, kind)
    }
}

/// An ordinary event's starting set: everything on your own event, the user's own settings on
/// someone else's (C14's allow-list), and the call and the calendar as C18 left them.
fn default_rights(facts: &EditFacts) -> FieldPermissions {
    let mut perms = FieldPermissions {
        summary: true,
        time: true,
        repeat: true,
        location: true,
        description: true,
        show_as: true,
        visibility: true,
        guests: true,
        reminders: true,
        colour: true,
        // Only Meet is made or removed here (the plan's CUT list), and added only where the calendar
        // offers it; a Meet call already there may still go, as Google web lets it (C18 critiques
        // C1-M1, C2-L14). A lock isn't Google's on the call.
        meet: matches!(facts.call, None | Some(CallKind::Meet))
            && (facts.meet_offered || facts.call == Some(CallKind::Meet)),
        // Another calendar of the same account (C18d): the save checks the one chosen.
        calendar: true,
        delete: true,
        // A lock is Google's on the shared fields: a reply isn't among them. Set by the caller.
        respond: false,
        // Any length of time: only a special date's is held to one day.
        one_day: false,
        reasons: Vec::new(),
    };
    if !facts.organizer_self {
        // Google: notifications, colour and busy/free are each guest's own (its inviting-attendees
        // guide); the rest is shared, and a guest's change to it shows on their own copy (A11), so
        // it changes only where the organiser lets guests change the event (Q16).
        if !facts.guests_can_modify {
            perms.summary = false;
            perms.time = false;
            perms.location = false;
            perms.description = false;
            perms.visibility = false;
            perms.reasons.push(ReadOnlyReason::NotOrganizer);
        }
        // How it repeats stays the organiser's whatever guests may change: a new rule on a guest's
        // copy would leave it apart from the organiser's event (C14 §0.5 row 5, critique C1-H2).
        perms.repeat = false;
        // Who is invited, and what guests may do, stay the organiser's in PM (Google: the three
        // permissions are organiser-only; a guest adding guests is a Backlog card, Q4).
        perms.guests = false;
        if facts.guests_can_modify {
            perms.reasons.push(ReadOnlyReason::OrganizerOnlyGuests);
        }
        // Taking the user's own copy off their calendar (C14d): Google tells the organiser they
        // declined, so it needs an entry on the guest list to decline from. A copy from a public
        // calendar has none and stays. A lock closes it below, as it does a delete.
        perms.delete = facts.invited;
        // A call is the organiser's, whatever they let guests change: one made on a guest's copy
        // would be a call nobody else sees (A11; C18 §0.5 row 7). The reason above names it.
        perms.meet = false;
        // So is its calendar: Google puts an event on another calendar from its organiser's copy
        // (C18d). The reason above names it too.
        perms.calendar = false;
    } else if matches!(facts.call, Some(CallKind::AddOn | CallKind::Other)) {
        // Shown and kept: Google's rule for a kind the client doesn't make (C18). On someone else's
        // event the organiser's reason already says the call is theirs.
        perms.reasons.push(ReadOnlyReason::OtherCall);
    }
    perms
}

/// What Google lets change on a birthday or a Gmail event (its event-types guide), and nothing
/// else. Every field starts closed, so a field a later layer adds stays closed here until it is
/// listed. Replaces the organiser rule for these kinds: Google fixes their organiser, and their own
/// list is the rule (R9).
fn special_rights(facts: &EditFacts, kind: Kind) -> FieldPermissions {
    let mut perms = FieldPermissions::default();
    match kind {
        Kind::SpecialDate => {
            perms.summary = true;
            perms.reminders = true;
            perms.colour = true;
            perms.delete = true;
            perms.one_day = true;
            // Whoever owns the day says so first, the kind's own line after it.
            match facts.date_source {
                DateSource::Calendar => perms.time = true,
                DateSource::Contacts => perms.reasons.push(ReadOnlyReason::DateFromContacts),
                DateSource::Profile => perms.reasons.push(ReadOnlyReason::DateFromProfile),
                DateSource::Unknown => {}
            }
            perms.reasons.push(ReadOnlyReason::SpecialDateFields);
        }
        Kind::FromGmail => {
            perms.show_as = true;
            perms.visibility = true;
            perms.reminders = true;
            perms.colour = true;
            perms.delete = true;
            perms.reasons.push(ReadOnlyReason::FromGmailFields);
        }
        // Never reached (see edit_rights); closed if it ever is.
        Kind::Default | Kind::Other => return FieldPermissions::none(ReadOnlyReason::SpecialType),
    }
    perms
}

/// What closes on top of a starting set, for every kind: Google's lock, formatting PM can't write
/// back, a label, notifications PM can't read.
fn closing_rules(mut perms: FieldPermissions, facts: &EditFacts, kind: Kind) -> FieldPermissions {
    if facts.locked {
        match kind {
            // Google's per-type guide is the more specific rule here (R9): "Birthday events can be
            // deleted", "Events from Gmail can be deleted", and a birthday's `summary` is on its
            // list, while the lock names "summary, description, location, start, end or
            // recurrence". The rest of the lock binds, and of what these kinds may change it holds
            // only a birthday's date. Said only when it closed that, so a locked Gmail event reads
            // the Gmail line (C20 critiques C2-H1, C1-M5).
            Kind::SpecialDate | Kind::FromGmail => {
                if perms.time {
                    perms.time = false;
                    perms.reasons.push(ReadOnlyReason::Locked);
                }
            }
            // Google refuses changes to a locked copy's summary, description, location, start, end
            // and recurrence. Whether it lets the copy be deleted, or its guests change, isn't
            // documented, so PM offers neither (INFERRED: fails closed, C12 §4.4). Whether it can go
            // to another calendar isn't documented either, so it stays on its own (C18d, §0.5 row
            // 11). Its notifications and colour aren't among them, so they stay open (the editing
            // plan's C6 card, from C13).
            Kind::Default | Kind::Other => {
                perms.summary = false;
                perms.time = false;
                perms.repeat = false;
                perms.location = false;
                perms.description = false;
                perms.guests = false;
                perms.calendar = false;
                perms.delete = false;
                perms.reasons.push(ReadOnlyReason::Locked);
            }
        }
    }
    // Only on a description otherwise open, so reopening it for a replace can never undo a lock or
    // the organiser's hold ([`FieldPermissions::replacing_formatting`]).
    if facts.unsupported_formatting && perms.description {
        perms.description = false;
        perms.reasons.push(ReadOnlyReason::UnsupportedFormatting);
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
/// allow it, the reply, and a drag where a move's own check would allow one (C16a). Computed from
/// the mirror's copy, so formatting PM can't edit (unknown there) never blocks it.
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
        can_retime: perms.time,
    }
}

/// A part of an event a write may be refused for, as [`reason_for`] names its reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Summary,
    Time,
    Repeat,
    Location,
    Description,
    ShowAs,
    Visibility,
    Guests,
    Reminders,
    Colour,
    /// The video call (C18).
    Meet,
    /// The calendar it's on (C18d).
    Calendar,
    Delete,
}

/// Reasons that close one field only: each is given for its own field alone, never for another
/// field it says nothing about (C14 critique C1-L1; C13's C1-L1 for the colour and notifications;
/// C18 critique C1-L6 for another service's call).
const ONE_FIELD: [ReadOnlyReason; 5] = [
    ReadOnlyReason::OrganizerOnlyGuests,
    ReadOnlyReason::UnsupportedFormatting,
    ReadOnlyReason::LabelReadOnly,
    ReadOnlyReason::RemindersUnreadable,
    ReadOnlyReason::OtherCall,
];

/// The reason to give when a write touches `field` and `perms` (from [`edit_rights`]) closed it: the
/// field's own reason when the gate closed it for that, else the first reason that isn't another
/// field's own. Every refusal says this, so a delete on an event whose organiser lets guests change
/// it never says "only the organiser can change who's invited" (C14 critique C1-L1).
pub fn reason_for(perms: &FieldPermissions, field: Field) -> ReadOnlyReason {
    if let Some(reason) = special_reason(perms, field) {
        return reason;
    }
    // The call's own first; under "modify" the organiser's guest reason covers it too (its words
    // name the call, C18).
    let own: &[ReadOnlyReason] = match field {
        Field::Guests => &[ReadOnlyReason::OrganizerOnlyGuests],
        Field::Description => &[ReadOnlyReason::UnsupportedFormatting],
        Field::Colour => &[ReadOnlyReason::LabelReadOnly],
        Field::Reminders => &[ReadOnlyReason::RemindersUnreadable],
        Field::Meet => &[
            ReadOnlyReason::OtherCall,
            ReadOnlyReason::OrganizerOnlyGuests,
        ],
        _ => &[],
    };
    if let Some(own) = own.iter().copied().find(|r| perms.reasons.contains(r)) {
        return own;
    }
    // Nothing else said: someone else's event, whose organiser lets guests change it, closes the
    // guest list with its own reason, and how it repeats and the delete with none of their own.
    perms
        .reasons
        .iter()
        .copied()
        .find(|r| !ONE_FIELD.contains(r))
        .unwrap_or(ReadOnlyReason::NotOrganizer)
}

/// The reason a closed `field` gives on a birthday or a Gmail event, read from `perms.reasons`
/// alone, or `None` for other events (the rules above then apply). Most specific first: whoever
/// owns the date, then Google's lock, then the field's own (a label, notifications PM can't read),
/// then the kind's list (C20 §4.1).
fn special_reason(perms: &FieldPermissions, field: Field) -> Option<ReadOnlyReason> {
    let kind = [
        ReadOnlyReason::SpecialDateFields,
        ReadOnlyReason::FromGmailFields,
    ]
    .into_iter()
    .find(|r| perms.reasons.contains(r))?;
    let first: &[ReadOnlyReason] = match field {
        Field::Time => &[
            ReadOnlyReason::DateFromContacts,
            ReadOnlyReason::DateFromProfile,
            ReadOnlyReason::Locked,
        ],
        Field::Summary | Field::Delete => &[ReadOnlyReason::Locked],
        Field::Reminders => &[ReadOnlyReason::RemindersUnreadable],
        Field::Colour => &[ReadOnlyReason::LabelReadOnly],
        _ => &[],
    };
    Some(
        first
            .iter()
            .copied()
            .find(|r| perms.reasons.contains(r))
            .unwrap_or(kind),
    )
}

/// Why `time` can't be saved under `perms`, or `None` when it can: a closed time gives its own
/// reason ([`reason_for`]), and a special date's time that isn't one whole day the kind's words,
/// which say what its date may be (C20 critiques C1-M2, C2-L7). One helper for every path that
/// sends a time: a save's fields, a drop, and a drop on an occurrence before it is planned.
pub fn time_refusal(perms: &FieldPermissions, time: &TimeDraft) -> Option<ReadOnlyReason> {
    if !perms.time {
        return Some(reason_for(perms, Field::Time));
    }
    (perms.one_day && !time.is_one_whole_day()).then_some(ReadOnlyReason::SpecialDateFields)
}

/// Why the user can't reply from this copy, or `None` when they can: the reasons nothing at all may
/// be written from it, then a birthday (Google keeps no guest list on one: `attendees` isn't on its
/// list), then an entry on the guest list. The organiser has one too, and may reply; a lock doesn't
/// stop it (Google locks the shared fields), and neither does a one-off Google has since made
/// repeating, which the reply's own read refuses as a save does. A Gmail event with an entry of the
/// user's own replies as any invitation does (`attendees` is on its list, C20 §3.3).
pub fn reply_block(facts: &EditFacts) -> Option<ReadOnlyReason> {
    access_reason(facts)
        .or((kind_of(facts.event_type) == Kind::SpecialDate)
            .then_some(ReadOnlyReason::SpecialDateFields))
        .or((!facts.invited).then_some(ReadOnlyReason::NotInvited))
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
        guests_can_modify: false,
        locked: false,
        visibility,
        series_master: false,
        unsupported_formatting: false,
        labelled: false,
        reminders_readable: true,
        invited: false,
        // A create's call is the create command's to check.
        call: None,
        meet_offered: false,
        // A create is `default` (PM creates no birthday, A16): unused.
        date_source: DateSource::Unknown,
    })
}

/// The reason nothing at all may change, checked account first, then calendar, then the event
/// itself; the first one found is the one shown. Someone else's event isn't one: what a guest may
/// change is [`edit_rights`]' allow-list (C14).
fn blanket_reason(facts: &EditFacts) -> Option<ReadOnlyReason> {
    if let Some(reason) = access_reason(facts) {
        return Some(reason);
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
    // Focus time, out of office, working location and types Google adds later stay read-only (Q8);
    // birthdays and Gmail events have Google's own list instead (R9, `special_rights`).
    if kind_of(facts.event_type) == Kind::Other {
        return Some(ReadOnlyReason::SpecialType);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use ReadOnlyReason::*;

    /// A solo, one-off event you organise on your own calendar, with editing on, on a calendar that
    /// offers Meet and with no call yet.
    fn editable() -> EditFacts<'static> {
        EditFacts {
            google: true,
            editing: EditingStatus::On,
            access_role: Some("owner"),
            event_type: Some("default"),
            organizer_self: true,
            guests_can_modify: false,
            locked: false,
            visibility: None,
            series_master: false,
            unsupported_formatting: false,
            labelled: false,
            reminders_readable: true,
            invited: false,
            call: None,
            meet_offered: true,
            date_source: DateSource::Unknown,
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
        // How it repeats is the organiser's, so yours (C14 critique C1-H2).
        assert!(perms.repeat);
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
            // Birthdays and Gmail events are no longer among them: Google's own list (C20).
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
            // Someone else's event is no longer one (C14's allow-list).
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
        // Google locks the rule with the start.
        assert!(!perms.repeat);
        // Undocumented, so it keeps its calendar (C18d).
        assert!(!perms.calendar);
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
    fn formatting_pm_cant_edit_is_read_only_and_says_why() {
        let perms = edit_rights(&EditFacts {
            unsupported_formatting: true,
            ..editable()
        });
        assert!(!perms.description);
        assert!(perms.summary && perms.time && perms.delete);
        assert_eq!(perms.reasons, vec![UnsupportedFormatting]);
        // Locked already covers the description, so it isn't given twice.
        let both = edit_rights(&EditFacts {
            unsupported_formatting: true,
            locked: true,
            ..editable()
        });
        assert_eq!(both.reasons, vec![Locked]);
    }

    /// C19 §8: "Replace with plain text…" reopens a description closed for its formatting, and
    /// nothing a lock or the organiser closed.
    #[test]
    fn replacing_formatting_reopens_only_its_own_reason() {
        let rows: [(&str, EditFacts, bool, bool, Vec<ReadOnlyReason>); 7] = [
            (
                "G1",
                EditFacts {
                    unsupported_formatting: true,
                    ..editable()
                },
                false,
                false,
                vec![UnsupportedFormatting],
            ),
            (
                "G2",
                EditFacts {
                    unsupported_formatting: true,
                    ..editable()
                },
                true,
                true,
                vec![],
            ),
            (
                "G3",
                EditFacts {
                    unsupported_formatting: true,
                    locked: true,
                    ..editable()
                },
                true,
                false,
                vec![Locked],
            ),
            (
                "G4",
                EditFacts {
                    unsupported_formatting: true,
                    ..invitation()
                },
                true,
                false,
                vec![NotOrganizer],
            ),
            ("G5", editable(), true, true, vec![]),
            // Someone else's event whose organiser lets guests change it: the description is theirs
            // to change, so a replace reopens it; the guest list stays the organiser's.
            (
                "G6",
                EditFacts {
                    unsupported_formatting: true,
                    guests_can_modify: true,
                    ..invitation()
                },
                true,
                true,
                vec![OrganizerOnlyGuests],
            ),
            // A calendar shown read-only: nothing reopens.
            (
                "G7",
                EditFacts {
                    unsupported_formatting: true,
                    access_role: Some("reader"),
                    ..editable()
                },
                true,
                false,
                vec![CalendarReadOnly],
            ),
        ];
        for (row, facts, replacing, description, reasons) in rows {
            let perms = edit_rights(&facts);
            let perms = if replacing {
                perms.replacing_formatting()
            } else {
                perms
            };
            assert_eq!(perms.description, description, "{row}: description");
            assert_eq!(perms.reasons, reasons, "{row}: reasons");
            // Nothing else moves: the rest is as the gate gave it.
            let gate = edit_rights(&facts);
            assert!(
                perms.summary == gate.summary
                    && perms.time == gate.time
                    && perms.guests == gate.guests
                    && perms.delete == gate.delete,
                "{row}: only the description reopens"
            );
        }
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

    /// A blanket reason closes notifications and colour too: nothing at all may change. (Someone
    /// else's event was the example until C14 made it one whose own settings change.)
    #[test]
    fn a_blanket_reason_closes_notifications_and_colour() {
        let perms = edit_rights(&EditFacts {
            access_role: Some("reader"),
            labelled: true,
            reminders_readable: false,
            ..editable()
        });
        assert!(!perms.reminders && !perms.colour && !perms.any());
        assert_eq!(perms.reasons, vec![CalendarReadOnly]);
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
                    event_type: Some("focusTime"),
                    ..i()
                },
                false,
                Some(SpecialType),
            ),
            // C20: a birthday has no guest list to answer from, whatever its copy says; a Gmail
            // event with an entry of the user's own replies as any invitation does.
            (
                "R4a",
                EditFacts {
                    event_type: Some("birthday"),
                    ..i()
                },
                false,
                Some(SpecialDateFields),
            ),
            (
                "R4b",
                EditFacts {
                    event_type: Some("fromGmail"),
                    ..i()
                },
                true,
                None,
            ),
            (
                "R4c",
                EditFacts {
                    event_type: Some("fromGmail"),
                    invited: false,
                    ..i()
                },
                false,
                Some(NotInvited),
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

    /// C14: where the reply is open and nothing in the editor is (a one-off of yours that Google has
    /// since made repeating, whose reply the reply's own read refuses), the list still says why the
    /// editor can't change it, offers no Delete, and offers the reply apart. (An invitation was the
    /// example until C14c opened its own settings: `an_invitation_is_editable_in_the_list`.)
    #[test]
    fn a_reply_alone_is_not_an_edit() {
        let turned = EditFacts {
            organizer_self: true,
            series_master: true,
            ..invitation()
        };
        let perms = edit_rights(&turned);
        assert!(!perms.any() && perms.respond && !perms.delete);
        assert_eq!(perms.reasons, vec![BecameRepeating]);
        assert!(
            list_rights(&turned)
                == ListRights {
                    edit_block: Some(BecameRepeating),
                    can_delete: false,
                    can_respond: true,
                    can_retime: false,
                },
            "no editor, no Delete, the reply"
        );
        // Your own event: Delete as the gate allows it, a reply only with an entry of your own, and
        // a drag (C16a).
        assert!(
            list_rights(&editable())
                == ListRights {
                    edit_block: None,
                    can_delete: true,
                    can_respond: false,
                    can_retime: true,
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
                    can_retime: false,
                },
            "a lock keeps the delete and the time closed and the reply open"
        );
    }

    /// C16 critique C2-M4: the list offers a drag exactly where the gate opens the time, so the view
    /// never offers one the move refuses: your own event, an invitation whose organiser lets guests
    /// change it; never a lock, someone else's event without that, or a special type.
    #[test]
    fn the_list_offers_a_drag_where_the_gate_opens_the_time() {
        let modify = EditFacts {
            guests_can_modify: true,
            ..invitation()
        };
        let special = EditFacts {
            event_type: Some("focusTime"),
            ..editable()
        };
        let locked = EditFacts {
            locked: true,
            ..editable()
        };
        // C20: the list never knows who owns a birthday's date, so it never offers its drag; a
        // fresh copy of one added in Google Calendar opens the time (to one whole day, `one_day`).
        let listed_date = EditFacts {
            event_type: Some("birthday"),
            ..editable()
        };
        let own_date = EditFacts {
            date_source: DateSource::Calendar,
            ..listed_date
        };
        let gmail = EditFacts {
            event_type: Some("fromGmail"),
            ..editable()
        };
        for (row, facts, retime) in [
            ("yours", editable(), true),
            ("modify", modify, true),
            ("theirs", invitation(), false),
            ("locked", locked, false),
            ("special", special, false),
            ("listed date", listed_date, false),
            ("own date", own_date, true),
            ("gmail", gmail, false),
        ] {
            assert_eq!(list_rights(&facts).can_retime, retime, "{row}");
            assert_eq!(edit_rights(&facts).time, retime, "{row}");
        }
    }

    /// C14c's allow-list, field by field (`G`: the organiser lets guests change it). Someone else's
    /// event: the user's own busy or free, notifications and colour always, the shared fields only
    /// with `G`, never how it repeats (C1-H2) or who is invited; the reply and Remove (C14d) wherever
    /// the calendar is on the guest list. Reasons in order, the organiser's first.
    #[test]
    fn the_allow_list_table() {
        let i = invitation;
        let with_g = |facts: EditFacts<'static>| EditFacts {
            guests_can_modify: true,
            ..facts
        };
        // (row, facts, title/time/place/description, visibility, busy/notifications/colour, repeat,
        // guests, delete, reply, reasons)
        type Row = (
            &'static str,
            EditFacts<'static>,
            [bool; 6],
            bool,
            Vec<ReadOnlyReason>,
        );
        let rows: Vec<Row> = vec![
            ("A1", editable(), [true; 6], false, vec![]),
            // C14d: Delete is Remove here, from a copy on the guest list; a lock still closes it (A4,
            // A5), and a copy with no entry has nothing to decline (A6).
            (
                "A2",
                i(),
                [false, false, true, false, false, true],
                true,
                vec![NotOrganizer],
            ),
            (
                "A3",
                with_g(i()),
                [true, true, true, false, false, true],
                true,
                vec![OrganizerOnlyGuests],
            ),
            (
                "A4",
                EditFacts {
                    locked: true,
                    ..i()
                },
                [false, false, true, false, false, false],
                true,
                vec![NotOrganizer, Locked],
            ),
            (
                "A5",
                EditFacts {
                    locked: true,
                    ..with_g(i())
                },
                [false, true, true, false, false, false],
                true,
                vec![OrganizerOnlyGuests, Locked],
            ),
            (
                "A6",
                EditFacts {
                    invited: false,
                    ..i()
                },
                [false, false, true, false, false, false],
                false,
                vec![NotOrganizer],
            ),
            (
                "A7",
                EditFacts {
                    event_type: Some("focusTime"),
                    ..i()
                },
                [false; 6],
                false,
                vec![SpecialType],
            ),
            (
                "A8",
                with_g(EditFacts {
                    invited: true,
                    ..editable()
                }),
                [true; 6],
                true,
                vec![],
            ),
            // The description is closed already, so its formatting isn't a second reason.
            (
                "A9",
                EditFacts {
                    unsupported_formatting: true,
                    ..i()
                },
                [false, false, true, false, false, true],
                true,
                vec![NotOrganizer],
            ),
            // With G the description is open but for its formatting: that reason, after the guests'.
            (
                "A10",
                EditFacts {
                    unsupported_formatting: true,
                    ..with_g(i())
                },
                [true, true, true, false, false, true],
                true,
                vec![OrganizerOnlyGuests, UnsupportedFormatting],
            ),
        ];
        for (row, facts, open, respond, reasons) in rows {
            let p = edit_rights(&facts);
            let [shared, visibility, own, repeat, guests, delete] = open;
            let description = shared && !facts.unsupported_formatting;
            assert_eq!(
                [p.summary, p.time, p.location],
                [shared; 3],
                "{row}: title, time, place"
            );
            assert_eq!(p.description, description, "{row}: description");
            assert_eq!(p.visibility, visibility, "{row}: visibility");
            assert_eq!(
                [p.show_as, p.reminders, p.colour],
                [own; 3],
                "{row}: busy or free, notifications, colour"
            );
            assert_eq!(p.repeat, repeat, "{row}: how it repeats");
            // The call is the organiser's as how it repeats is, and these rows lock neither (C18).
            assert_eq!(p.meet, repeat, "{row}: video call");
            // So is the calendar, and a lock closes both (C18d).
            assert_eq!(p.calendar, repeat, "{row}: calendar");
            assert_eq!(p.guests, guests, "{row}: guests");
            assert_eq!(p.delete, delete, "{row}: delete");
            assert_eq!(p.respond, respond, "{row}: reply");
            assert_eq!(p.reasons, reasons, "{row}: reasons");
        }
    }

    /// C14c: an invitation is something the editor can change (its own settings), so the list
    /// offers Edit with no reason and the reply. C14d: and Remove, which the list marks as Delete.
    #[test]
    fn an_invitation_is_editable_in_the_list() {
        for (facts, retime) in [
            (invitation(), false),
            (
                EditFacts {
                    guests_can_modify: true,
                    ..invitation()
                },
                true,
            ),
        ] {
            assert!(
                list_rights(&facts)
                    == ListRights {
                        edit_block: None,
                        can_delete: true,
                        can_respond: true,
                        can_retime: retime,
                    },
                "editable, Remove, the reply, a drag only where guests may change it"
            );
        }
    }

    /// C14d, C20 §0.4 item 11: a delete only takes the copy off the calendar on an ordinary event
    /// someone else organises. Your own event is deleted; a birthday's and a Gmail event's organiser
    /// is Google's, whoever it names, so they are deleted with no decline; a kind PM doesn't know
    /// fails closed (the gate refuses it anyway).
    #[test]
    fn only_someone_elses_ordinary_event_is_removed_from_a_calendar() {
        for event_type in [None, Some("default")] {
            assert!(removes_copy(event_type, false), "{event_type:?}");
            assert!(!removes_copy(event_type, true), "{event_type:?}: yours");
        }
        for event_type in ["birthday", "fromGmail", "focusTime", "somethingNew"] {
            assert!(!removes_copy(Some(event_type), false), "{event_type}");
            assert!(!removes_copy(Some(event_type), true), "{event_type}");
        }
    }

    /// C14d: what a removal asks Google to send, which is what the L-I4 live test checks.
    #[test]
    fn a_removal_tells_the_organiser() {
        assert_eq!(REMOVAL_NOTIFY.as_param(), "all");
    }

    /// C14 critique C1-L1: a refusal names the refused field's own reason, never another field's: the
    /// guest list's under "modify", a label's for the colour, a formatted description's, and for a
    /// delete or a repeat on someone else's event the organiser's.
    #[test]
    fn a_refusal_names_the_fields_own_reason() {
        let rights = |facts: EditFacts| edit_rights(&facts);
        let g = rights(EditFacts {
            guests_can_modify: true,
            ..invitation()
        });
        assert_eq!(reason_for(&g, Field::Guests), OrganizerOnlyGuests);
        assert_eq!(reason_for(&g, Field::Delete), NotOrganizer);
        assert_eq!(reason_for(&g, Field::Repeat), NotOrganizer);
        let g_locked = rights(EditFacts {
            guests_can_modify: true,
            locked: true,
            ..invitation()
        });
        assert_eq!(reason_for(&g_locked, Field::Summary), Locked);
        assert_eq!(reason_for(&g_locked, Field::Delete), Locked);
        assert_eq!(reason_for(&g_locked, Field::Guests), OrganizerOnlyGuests);
        let g_formatted = rights(EditFacts {
            guests_can_modify: true,
            unsupported_formatting: true,
            ..invitation()
        });
        assert_eq!(
            reason_for(&g_formatted, Field::Description),
            UnsupportedFormatting
        );
        assert_eq!(reason_for(&g_formatted, Field::Delete), NotOrganizer);
        let plain = rights(invitation());
        for field in [Field::Guests, Field::Summary, Field::Repeat, Field::Delete] {
            assert_eq!(reason_for(&plain, field), NotOrganizer, "{field:?}");
        }
        // Your own event: a lock for the repeat, a label for the colour (even under a lock).
        let locked = rights(EditFacts {
            locked: true,
            labelled: true,
            ..editable()
        });
        assert_eq!(reason_for(&locked, Field::Repeat), Locked);
        assert_eq!(reason_for(&locked, Field::Colour), LabelReadOnly);
        assert_eq!(reason_for(&locked, Field::Time), Locked);
        let unreadable = rights(EditFacts {
            reminders_readable: false,
            ..editable()
        });
        assert_eq!(
            reason_for(&unreadable, Field::Reminders),
            RemindersUnreadable
        );
        // C18: the call's own reason for the call alone (critique C1-L6): a locked event with another
        // service's call refuses a title with the lock's words, the call with its own.
        let zoom_locked = rights(EditFacts {
            locked: true,
            call: Some(CallKind::AddOn),
            ..editable()
        });
        assert_eq!(reason_for(&zoom_locked, Field::Meet), OtherCall);
        assert_eq!(reason_for(&zoom_locked, Field::Summary), Locked);
        assert_eq!(reason_for(&zoom_locked, Field::Delete), Locked);
        // Someone else's event: the organiser's reason, "modify" or not.
        assert_eq!(reason_for(&g, Field::Meet), OrganizerOnlyGuests);
        assert_eq!(reason_for(&plain, Field::Meet), NotOrganizer);
    }

    /// C18: who may add or remove a Google Meet call, and why not (§3.3, with critique C1-M1's
    /// row G12): Meet only, the organiser's, added only where the calendar offers it; a Meet call
    /// already there may still go. Another service's call is shown and kept, with its reason, on
    /// the organiser's event; on someone else's the organiser's reason covers it.
    #[test]
    fn the_call_row_table() {
        let organiser = |call, meet_offered| EditFacts {
            call,
            meet_offered,
            ..editable()
        };
        let rows: Vec<(&str, EditFacts, bool, Vec<ReadOnlyReason>)> = vec![
            ("G1", organiser(None, true), true, vec![]),
            ("G2", organiser(Some(CallKind::Meet), true), true, vec![]),
            (
                "G3",
                organiser(Some(CallKind::AddOn), true),
                false,
                vec![OtherCall],
            ),
            (
                "G4",
                organiser(Some(CallKind::Other), true),
                false,
                vec![OtherCall],
            ),
            ("G5", organiser(None, false), false, vec![]),
            (
                "G6",
                organiser(Some(CallKind::AddOn), false),
                false,
                vec![OtherCall],
            ),
            (
                "G7",
                EditFacts {
                    locked: true,
                    ..organiser(None, true)
                },
                true,
                vec![Locked],
            ),
            ("G8", invitation(), false, vec![NotOrganizer]),
            (
                "G9",
                EditFacts {
                    guests_can_modify: true,
                    ..invitation()
                },
                false,
                vec![OrganizerOnlyGuests],
            ),
            (
                "G10",
                EditFacts {
                    event_type: Some("focusTime"),
                    ..editable()
                },
                false,
                vec![SpecialType],
            ),
            // C20: a call isn't on a birthday's list, whatever the calendar offers.
            (
                "G10a",
                EditFacts {
                    event_type: Some("birthday"),
                    date_source: DateSource::Calendar,
                    ..editable()
                },
                false,
                vec![SpecialDateFields],
            ),
            (
                "G11",
                EditFacts {
                    access_role: Some("writerWithoutPrivateAccess"),
                    ..editable()
                },
                true,
                vec![],
            ),
            ("G12", organiser(Some(CallKind::Meet), false), true, vec![]),
            // Someone else's event with another service's call: the organiser's reason alone.
            (
                "G13",
                EditFacts {
                    call: Some(CallKind::AddOn),
                    ..invitation()
                },
                false,
                vec![NotOrganizer],
            ),
            (
                "G14",
                EditFacts {
                    locked: true,
                    ..organiser(Some(CallKind::AddOn), true)
                },
                false,
                vec![OtherCall, Locked],
            ),
        ];
        for (row, facts, meet, reasons) in rows {
            let p = edit_rights(&facts);
            assert_eq!(p.meet, meet, "{row}: the call");
            assert_eq!(p.reasons, reasons, "{row}: reasons");
        }
        // A calendar without Meet still leaves the rest of the event to change (G5).
        assert!(edit_rights(&organiser(None, false)).any());
        // The call alone is something the editor can change.
        let only = edit_rights(&EditFacts {
            locked: true,
            labelled: true,
            reminders_readable: false,
            ..editable()
        });
        assert!(only.meet);
    }

    /// C18d: who may put an event on another calendar, and why not (§3.3): the organiser, on a
    /// calendar they can edit; never a locked event, someone else's (whatever they let guests
    /// change) or a special kind. A refusal names the organiser's reason, never the guest list's.
    #[test]
    fn the_calendar_row_table() {
        let rows: Vec<(&str, EditFacts, bool, Vec<ReadOnlyReason>)> = vec![
            ("T1", editable(), true, vec![]),
            (
                "T2",
                EditFacts {
                    locked: true,
                    ..editable()
                },
                false,
                vec![Locked],
            ),
            ("T3", invitation(), false, vec![NotOrganizer]),
            (
                "T4",
                EditFacts {
                    guests_can_modify: true,
                    ..invitation()
                },
                false,
                vec![OrganizerOnlyGuests],
            ),
            (
                "T5",
                EditFacts {
                    event_type: Some("focusTime"),
                    ..editable()
                },
                false,
                vec![SpecialType],
            ),
            // C20: neither is another calendar on a Gmail event's (`events.move` takes default
            // events only).
            (
                "T5a",
                EditFacts {
                    event_type: Some("fromGmail"),
                    ..editable()
                },
                false,
                vec![FromGmailFields],
            ),
            (
                "T6",
                EditFacts {
                    editing: EditingStatus::Off,
                    ..editable()
                },
                false,
                vec![EditingOff],
            ),
            (
                "T7",
                EditFacts {
                    access_role: Some("writerWithoutPrivateAccess"),
                    ..editable()
                },
                true,
                vec![],
            ),
            (
                "T8",
                EditFacts {
                    access_role: Some("reader"),
                    ..editable()
                },
                false,
                vec![CalendarReadOnly],
            ),
        ];
        for (row, facts, calendar, reasons) in rows {
            let p = edit_rights(&facts);
            assert_eq!(p.calendar, calendar, "{row}: the calendar");
            assert_eq!(p.reasons, reasons, "{row}: reasons");
        }
        // The calendar alone is something the editor can change.
        assert!(FieldPermissions {
            calendar: true,
            ..Default::default()
        }
        .any());
        let rights = |facts: EditFacts| edit_rights(&facts);
        assert_eq!(
            reason_for(
                &rights(EditFacts {
                    locked: true,
                    ..editable()
                }),
                Field::Calendar
            ),
            Locked
        );
        assert_eq!(
            reason_for(&rights(invitation()), Field::Calendar),
            NotOrganizer
        );
        // "Modify" lets guests change the event, never its calendar: the organiser's words, which
        // name it, not the guest list's.
        assert_eq!(
            reason_for(
                &rights(EditFacts {
                    guests_can_modify: true,
                    ..invitation()
                }),
                Field::Calendar
            ),
            NotOrganizer
        );
    }

    /// C20 §1: Google's names, the absent one its documented default; anything else, a new type
    /// or another spelling, is read-only.
    #[test]
    fn kinds_read_as_google_names_them() {
        for (event_type, kind) in [
            (None, Kind::Default),
            (Some("default"), Kind::Default),
            (Some("birthday"), Kind::SpecialDate),
            (Some("fromGmail"), Kind::FromGmail),
            (Some("focusTime"), Kind::Other),
            (Some("outOfOffice"), Kind::Other),
            (Some("workingLocation"), Kind::Other),
            (Some("somethingNew"), Kind::Other),
            (Some("Birthday"), Kind::Other),
            (Some("fromgmail"), Kind::Other),
        ] {
            assert_eq!(kind_of(event_type), kind, "{event_type:?}");
        }
    }

    /// C20 §1 and critique C1-L7: who owns a special date's day, as Google's events reference
    /// documents `birthdayProperties`; whatever PM can't read for sure stays put.
    #[test]
    fn a_special_dates_source_reads_as_google_documents() {
        use serde_json::json;
        let of = |props: Option<Value>| {
            let mut event = json!({ "eventType": "birthday" });
            if let Some(props) = props {
                event["birthdayProperties"] = props;
            }
            date_source_of(&event)
        };
        let rows: Vec<(&str, Option<Value>, DateSource)> = vec![
            ("D1", None, DateSource::Unknown),
            ("D2", Some(json!("x")), DateSource::Unknown),
            ("D3", Some(json!({})), DateSource::Calendar),
            (
                "D4",
                Some(json!({ "type": "birthday" })),
                DateSource::Calendar,
            ),
            (
                "D5",
                Some(json!({ "type": "birthday", "contact": "" })),
                DateSource::Calendar,
            ),
            (
                "D6",
                Some(json!({ "type": "birthday", "contact": "people/c1" })),
                DateSource::Contacts,
            ),
            (
                "D7",
                Some(json!({ "contact": "people/c1" })),
                DateSource::Contacts,
            ),
            (
                "D8",
                Some(json!({ "type": "anniversary", "contact": "people/c1" })),
                DateSource::Contacts,
            ),
            (
                "D9",
                Some(
                    json!({ "type": "custom", "customTypeName": "Name day", "contact": "people/c1" }),
                ),
                DateSource::Contacts,
            ),
            (
                "D10",
                Some(json!({ "type": "other" })),
                DateSource::Contacts,
            ),
            ("D11", Some(json!({ "type": "self" })), DateSource::Profile),
            (
                "D12",
                Some(json!({ "type": "self", "contact": "people/c1" })),
                DateSource::Profile,
            ),
            (
                "D13",
                Some(json!({ "type": "newKind" })),
                DateSource::Unknown,
            ),
            ("D14", Some(json!({ "type": 5 })), DateSource::Unknown),
            ("D15", Some(json!({ "contact": 5 })), DateSource::Unknown),
            (
                "D16",
                Some(json!({ "type": "birthday", "contact": {} })),
                DateSource::Unknown,
            ),
            // A contact Google sent as null is none.
            (
                "D17",
                Some(json!({ "type": "birthday", "contact": null })),
                DateSource::Calendar,
            ),
            ("D18", Some(json!({ "type": null })), DateSource::Unknown),
        ];
        for (row, props, want) in rows {
            assert_eq!(of(props), want, "{row}");
        }
    }

    /// A birthday or other special date on your own calendar, as a fresh copy says it: `source`
    /// owns its day.
    fn special_date(source: DateSource) -> EditFacts<'static> {
        EditFacts {
            event_type: Some("birthday"),
            date_source: source,
            ..editable()
        }
    }

    /// An event Gmail added to your own calendar, organised by it, the calendar not on its list.
    fn from_gmail() -> EditFacts<'static> {
        EditFacts {
            event_type: Some("fromGmail"),
            ..editable()
        }
    }

    /// C20 §3.4, with the Adopted list's flips (C2-H1, C1-M5: a lock leaves a birthday's name and
    /// both kinds' delete open, and is said only where it closed something the kind had open).
    /// `[summary, time, location and description, busy or free, visibility, notifications, colour,
    /// delete, one_day, reply]`; how it repeats, guests, the call and the calendar are closed on
    /// every row.
    #[test]
    fn the_special_kinds_table() {
        let s1 = || special_date(DateSource::Calendar);
        let gmail_invited = || EditFacts {
            organizer_self: false,
            invited: true,
            ..from_gmail()
        };
        const NONE: [bool; 10] = [false; 10];
        let date = |time: bool| {
            [
                true, time, false, false, false, true, true, true, true, false,
            ]
        };
        let gmail = |reply: bool| {
            [
                false, false, false, true, true, true, true, true, false, reply,
            ]
        };
        type Row = (
            &'static str,
            EditFacts<'static>,
            [bool; 10],
            Vec<ReadOnlyReason>,
        );
        let mut rows: Vec<Row> = vec![
            ("S1", s1(), date(true), vec![SpecialDateFields]),
            (
                "S2",
                special_date(DateSource::Contacts),
                date(false),
                vec![DateFromContacts, SpecialDateFields],
            ),
            (
                "S3",
                special_date(DateSource::Profile),
                date(false),
                vec![DateFromProfile, SpecialDateFields],
            ),
            (
                "S4",
                special_date(DateSource::Unknown),
                date(false),
                vec![SpecialDateFields],
            ),
            (
                "S5",
                EditFacts {
                    locked: true,
                    ..s1()
                },
                date(false),
                vec![SpecialDateFields, Locked],
            ),
            // The lock closed nothing a contact's birthday had open: not said.
            (
                "S5a",
                EditFacts {
                    locked: true,
                    ..special_date(DateSource::Contacts)
                },
                date(false),
                vec![DateFromContacts, SpecialDateFields],
            ),
            (
                "S6",
                EditFacts {
                    organizer_self: false,
                    ..s1()
                },
                date(true),
                vec![SpecialDateFields],
            ),
            (
                "S7",
                EditFacts {
                    invited: true,
                    ..s1()
                },
                date(true),
                vec![SpecialDateFields],
            ),
            ("S8", from_gmail(), gmail(false), vec![FromGmailFields]),
            ("S9", gmail_invited(), gmail(true), vec![FromGmailFields]),
            (
                "S10",
                EditFacts {
                    locked: true,
                    ..from_gmail()
                },
                gmail(false),
                vec![FromGmailFields],
            ),
            (
                "S11",
                EditFacts {
                    labelled: true,
                    ..from_gmail()
                },
                [
                    false, false, false, true, true, true, false, true, false, false,
                ],
                vec![FromGmailFields, LabelReadOnly],
            ),
            (
                "S12",
                EditFacts {
                    reminders_readable: false,
                    ..s1()
                },
                [
                    true, true, false, false, false, false, true, true, true, false,
                ],
                vec![SpecialDateFields, RemindersUnreadable],
            ),
            (
                "S13",
                EditFacts {
                    guests_can_modify: true,
                    ..gmail_invited()
                },
                gmail(true),
                vec![FromGmailFields],
            ),
            (
                "S15",
                EditFacts {
                    access_role: Some("reader"),
                    ..s1()
                },
                NONE,
                vec![CalendarReadOnly],
            ),
            (
                "S16",
                EditFacts {
                    editing: EditingStatus::Off,
                    ..from_gmail()
                },
                NONE,
                vec![EditingOff],
            ),
            (
                "S17",
                EditFacts {
                    access_role: Some("writerWithoutPrivateAccess"),
                    visibility: Some("private"),
                    ..s1()
                },
                NONE,
                vec![PrivateEvent],
            ),
            // Another service's call is no reason here: the call isn't on the list at all (C18's
            // push stays the ordinary event's, critique C1-L8).
            (
                "S18",
                EditFacts {
                    call: Some(CallKind::AddOn),
                    ..s1()
                },
                date(true),
                vec![SpecialDateFields],
            ),
            // Formatting PM can't edit closes nothing more: the description is closed already.
            (
                "S19",
                EditFacts {
                    unsupported_formatting: true,
                    ..from_gmail()
                },
                gmail(false),
                vec![FromGmailFields],
            ),
        ];
        for event_type in [
            "focusTime",
            "outOfOffice",
            "workingLocation",
            "somethingNew",
        ] {
            rows.push((
                "S14",
                EditFacts {
                    event_type: Some(event_type),
                    invited: true,
                    ..editable()
                },
                NONE,
                vec![SpecialType],
            ));
        }
        for (row, facts, open, reasons) in rows {
            let p = edit_rights(&facts);
            let [summary, time, place, show_as, visibility, reminders, colour, delete, one_day, reply] =
                open;
            assert_eq!(p.summary, summary, "{row}: title");
            assert_eq!(p.time, time, "{row}: time");
            assert_eq!(
                [p.location, p.description],
                [place; 2],
                "{row}: place, description"
            );
            assert_eq!(p.show_as, show_as, "{row}: busy or free");
            assert_eq!(p.visibility, visibility, "{row}: visibility");
            assert_eq!(p.reminders, reminders, "{row}: notifications");
            assert_eq!(p.colour, colour, "{row}: colour");
            assert_eq!(
                [p.repeat, p.guests, p.meet, p.calendar],
                [false; 4],
                "{row}: repeat, guests, call, calendar"
            );
            assert_eq!(p.delete, delete, "{row}: delete");
            assert_eq!(p.one_day, one_day, "{row}: one day");
            assert_eq!(p.respond, reply, "{row}: reply");
            assert_eq!(p.reasons, reasons, "{row}: reasons");
        }
    }

    /// C20 §3.4: every field named, no `..`, so a field a later layer adds fails to compile here
    /// until its value on a birthday and a Gmail event is stated (risk 2).
    #[test]
    fn every_field_has_a_rule_for_the_special_kinds() {
        let FieldPermissions {
            summary,
            time,
            repeat,
            location,
            description,
            show_as,
            visibility,
            guests,
            reminders,
            colour,
            meet,
            calendar,
            delete,
            respond,
            one_day,
            reasons,
        } = edit_rights(&special_date(DateSource::Calendar));
        assert!(summary && time && reminders && colour && delete && one_day);
        assert!(!repeat && !location && !description && !show_as && !visibility && !guests);
        assert!(!meet && !calendar && !respond);
        assert_eq!(reasons, vec![SpecialDateFields]);
        let FieldPermissions {
            summary,
            time,
            repeat,
            location,
            description,
            show_as,
            visibility,
            guests,
            reminders,
            colour,
            meet,
            calendar,
            delete,
            respond,
            one_day,
            reasons,
        } = edit_rights(&from_gmail());
        assert!(show_as && visibility && reminders && colour && delete);
        assert!(!summary && !time && !repeat && !location && !description && !guests);
        assert!(!meet && !calendar && !respond && !one_day);
        assert_eq!(reasons, vec![FromGmailFields]);
    }

    /// C20 §3.4, critique C1-L10: the list offers what the gate does, field by field.
    #[test]
    fn the_list_offers_what_the_gate_does_on_special_kinds() {
        let listed = list_rights(&special_date(DateSource::Unknown));
        assert_eq!(listed.edit_block, None, "a birthday opens Edit");
        assert!(listed.can_delete && !listed.can_respond && !listed.can_retime);
        let invited = list_rights(&EditFacts {
            organizer_self: false,
            invited: true,
            ..from_gmail()
        });
        assert_eq!(invited.edit_block, None);
        assert!(invited.can_delete && invited.can_respond && !invited.can_retime);
        let focus = list_rights(&EditFacts {
            event_type: Some("focusTime"),
            invited: true,
            ..editable()
        });
        assert_eq!(focus.edit_block, Some(SpecialType));
        assert!(!focus.can_delete && !focus.can_respond && !focus.can_retime);
    }

    /// C20 §4.1: on these kinds a refusal names whoever owns the date first, then Google's lock,
    /// then the field's own reason, then the kind's list.
    #[test]
    fn a_special_kinds_refusal_names_the_most_specific_reason() {
        let rights = |facts: EditFacts| edit_rights(&facts);
        let contacts = rights(special_date(DateSource::Contacts));
        assert_eq!(reason_for(&contacts, Field::Time), DateFromContacts);
        assert_eq!(reason_for(&contacts, Field::Location), SpecialDateFields);
        let locked_contacts = rights(EditFacts {
            locked: true,
            ..special_date(DateSource::Contacts)
        });
        assert_eq!(reason_for(&locked_contacts, Field::Time), DateFromContacts);
        let profile = rights(special_date(DateSource::Profile));
        assert_eq!(reason_for(&profile, Field::Time), DateFromProfile);
        let listed = rights(special_date(DateSource::Unknown));
        assert_eq!(reason_for(&listed, Field::Time), SpecialDateFields);
        let locked = rights(EditFacts {
            locked: true,
            ..special_date(DateSource::Calendar)
        });
        assert_eq!(reason_for(&locked, Field::Time), Locked);
        // The kind's list, for what isn't on it, the lock beside it or not.
        for field in [
            Field::Location,
            Field::Description,
            Field::Repeat,
            Field::Guests,
            Field::Meet,
            Field::Calendar,
            Field::ShowAs,
            Field::Visibility,
        ] {
            assert_eq!(reason_for(&locked, field), SpecialDateFields, "{field:?}");
        }
        let gmail = rights(from_gmail());
        for field in [Field::Summary, Field::Time, Field::Repeat, Field::Guests] {
            assert_eq!(reason_for(&gmail, field), FromGmailFields, "{field:?}");
        }
        // A lock that closed nothing the kind had open isn't said, so not given either.
        let gmail_locked = rights(EditFacts {
            locked: true,
            ..from_gmail()
        });
        assert_eq!(reason_for(&gmail_locked, Field::Time), FromGmailFields);
        let labelled = rights(EditFacts {
            labelled: true,
            ..from_gmail()
        });
        assert_eq!(reason_for(&labelled, Field::Colour), LabelReadOnly);
        assert_eq!(reason_for(&labelled, Field::Summary), FromGmailFields);
        let unreadable = rights(EditFacts {
            reminders_readable: false,
            ..special_date(DateSource::Calendar)
        });
        assert_eq!(
            reason_for(&unreadable, Field::Reminders),
            RemindersUnreadable
        );
        assert_eq!(reason_for(&unreadable, Field::Location), SpecialDateFields);
        // Should Google refuse a locked birthday's rename (live probe B10), its title and its delete
        // close under the lock and say so: the precedence holds for any rights with both reasons.
        let both = FieldPermissions {
            reasons: vec![SpecialDateFields, Locked],
            ..Default::default()
        };
        assert_eq!(reason_for(&both, Field::Summary), Locked);
        assert_eq!(reason_for(&both, Field::Delete), Locked);
        assert_eq!(reason_for(&both, Field::Location), SpecialDateFields);
        // Other events keep C14's rules: a guest list's own reason under "modify".
        let modify = rights(EditFacts {
            guests_can_modify: true,
            ..invitation()
        });
        assert_eq!(reason_for(&modify, Field::Guests), OrganizerOnlyGuests);
    }

    /// C20 critiques C1-M2, C2-L7: one helper for every path that sends a time.
    #[test]
    fn a_time_is_refused_for_its_reason_or_for_not_being_one_whole_day() {
        let all_day = |first: &str, last: &str| TimeDraft::AllDay {
            first_day: first.into(),
            last_day: last.into(),
        };
        let timed = TimeDraft::Timed {
            start_date: "2026-05-04".into(),
            start_time: "09:00".into(),
            start_zone: "Europe/London".into(),
            end_date: "2026-05-04".into(),
            end_time: "10:00".into(),
            end_zone: "Europe/London".into(),
        };
        let own = edit_rights(&special_date(DateSource::Calendar));
        assert_eq!(
            time_refusal(&own, &all_day("2026-06-05", "2026-06-05")),
            None
        );
        assert_eq!(
            time_refusal(&own, &all_day("2026-06-05", "2026-06-06")),
            Some(SpecialDateFields)
        );
        assert_eq!(time_refusal(&own, &timed), Some(SpecialDateFields));
        let contacts = edit_rights(&special_date(DateSource::Contacts));
        assert_eq!(
            time_refusal(&contacts, &all_day("2026-06-05", "2026-06-05")),
            Some(DateFromContacts)
        );
        // Any length on an ordinary event.
        let ordinary = edit_rights(&editable());
        assert_eq!(
            time_refusal(&ordinary, &all_day("2026-06-05", "2026-06-07")),
            None
        );
        assert_eq!(time_refusal(&ordinary, &timed), None);
        assert_eq!(
            time_refusal(&edit_rights(&invitation()), &timed),
            Some(NotOrganizer)
        );
    }
}
