// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What a calendar edit is made of, and what a save returns. The webview sends drafts that name only
//! what the user touched; it never sends a provider id, an etag or a raw Google field (those stay in
//! the backend's edit session). Outcomes come back as data (`Ok(WriteOutcome)`), so the editor can
//! keep the draft and explain, rather than read an error string.

use serde::{Deserialize, Serialize};

use crate::calendar_recur::wire::RepeatRule;

/// Who Google emails about a change (`sendUpdates`). Every write states it; PM never relies on
/// Google's default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Notify {
    All,
    ExternalOnly,
    None,
}

impl Notify {
    /// The `sendUpdates` query value.
    pub fn as_param(self) -> &'static str {
        match self {
            Notify::All => "all",
            Notify::ExternalOnly => "externalOnly",
            Notify::None => "none",
        }
    }
}

/// How the time reads on the calendar: Google's `transparency`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShowAs {
    Busy,
    Free,
}

impl ShowAs {
    pub fn as_transparency(self) -> &'static str {
        match self {
            ShowAs::Busy => "opaque",
            ShowAs::Free => "transparent",
        }
    }
}

/// Who can see the event's details: Google's `visibility`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Visibility {
    Default,
    Public,
    Private,
    Confidential,
}

impl Visibility {
    pub fn as_param(self) -> &'static str {
        match self {
            Visibility::Default => "default",
            Visibility::Public => "public",
            Visibility::Private => "private",
            Visibility::Confidential => "confidential",
        }
    }
}

/// When the event happens, as the user set it in the editor: wall-clock times in a named zone, or
/// whole days. The zone is always explicit (no UTC fallback anywhere, rule R6): an event keeps its
/// own zone, and its end may be in another (a flight). Dates are `YYYY-MM-DD` and times `HH:MM`, as
/// the editor's fields hold them; [`super::time::resolve`] parses and checks them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TimeDraft {
    Timed {
        start_date: String,
        start_time: String,
        start_zone: String,
        end_date: String,
        end_time: String,
        end_zone: String,
    },
    /// Whole days. `last_day` is the last day the event covers, as people say it ("10–12 Oct");
    /// Google's `end.date` is the day after, which [`super::time`] converts.
    AllDay { first_day: String, last_day: String },
}

impl TimeDraft {
    /// One whole day: all day, first and last day the same (what a birthday's date may be, C20).
    pub fn is_one_whole_day(&self) -> bool {
        matches!(self, TimeDraft::AllDay { first_day, last_day } if first_day == last_day)
    }
}

/// The fields the user touched in the editor. `None` means untouched and is never sent; `Some` of
/// the value already on the event is dropped too, so nothing Google holds is rewritten for nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventPatchDraft {
    pub summary: Option<String>,
    /// `Some("")` clears it.
    pub location: Option<String>,
    /// Plain lines, or formatting as PM's editor writes it (Google's own subset). `Some("")` clears
    /// it.
    pub description: Option<String>,
    /// The user chose "Replace with plain text…": a description whose formatting PM can't edit may
    /// change. Reopens only that reason ([`FieldPermissions::replacing_formatting`]), and only for a
    /// draft that sends a description. `Debug` carries it into the choice token's bind, so a token
    /// asked for without it never lets a save replace formatting.
    #[serde(default)]
    pub replace_formatting: bool,
    pub time: Option<TimeDraft>,
    pub show_as: Option<ShowAs>,
    pub visibility: Option<Visibility>,
    /// How it repeats from now on. `None` leaves the repeat as it is (C11).
    pub recurrence: Option<RepeatDraft>,
    /// Guests the user added, took off or marked optional, as changes to Google's own list (R1);
    /// never the list itself. `Some` of an empty list reads as `None` (C12).
    pub guests: Option<Vec<GuestOp>>,
    /// Google's `guestsCanModify`, `guestsCanInviteOthers` and `guestsCanSeeOtherGuests`.
    pub guests_can_modify: Option<bool>,
    pub guests_can_invite_others: Option<bool>,
    pub guests_can_see_other_guests: Option<bool>,
    /// Its notifications: the whole list, or the calendar's defaults (C13).
    pub reminders: Option<RemindersDraft>,
    /// Its colour (C13).
    pub colour: Option<ColourDraft>,
    /// Add or remove its Google Meet call (C18). `None` leaves the call as it is.
    pub meet: Option<MeetChange>,
    /// The id of the request for a new call (`createRequest.requestId`): the editor session's, put in
    /// by the backend before planning, never by the webview (R5). One per session, so the choices,
    /// the save and a resend after a lost answer ask for one call, which Google makes once. `Debug`
    /// carries it into the choice token's bind, so a token binds the very call asked for.
    #[serde(skip)]
    pub meet_request: Option<String>,
    /// Another calendar of the same Google account to put the event on, by PM's calendar id (C18d).
    /// `None`, or the event's own calendar, leaves it where it is. Never a body key: it goes after
    /// the field changes, as a request of its own.
    pub calendar: Option<String>,
}

/// Adding or removing a Google Meet call, as the editor's video-call row sends it. Only Meet: PM never
/// makes another service's call (the editing plan's CUT list).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MeetChange {
    Add,
    Remove,
}

/// Whose video call an event has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CallKind {
    /// Google Meet (`hangoutsMeet`).
    Meet,
    /// Another service's, through a Calendar add-on (`addOn`): shown, never changed.
    AddOn,
    /// A kind Google no longer makes (`eventHangout`, `eventNamedHangout`), an empty or unknown kind,
    /// or a bare `hangoutLink` with no call details: shown, never changed (Google's rule for
    /// unfamiliar kinds).
    Other,
}

/// How far Google has got with the call (`createRequest.status.statusCode`; absent is ready).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CallState {
    Ready,
    Pending,
    Failed,
}

/// The event's video call as the editor shows it, from Google's fresh copy. Never its phone details
/// or notes: the notes may be HTML, and the phone entries carry PINs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CallForEdit {
    pub kind: CallKind,
    /// The service's name as Google gives it ("Google Meet", "Zoom Meeting"), at most 100
    /// characters; "" when Google gives none.
    pub name: String,
    /// The link to join by video: an `https://` entry point, else an `https://` `hangoutLink`. `None`
    /// while there is none.
    pub join: Option<String>,
    pub state: CallState,
}

/// An event's notifications, as the editor holds them and as a save sends them: whole, since Google
/// replaces a list it is sent (C13). `use_default` is the calendar's own defaults, `overrides` then
/// empty. One Google holds that PM wouldn't add itself (an old method, a time outside Google's
/// documented range) goes back as it is unless the user removes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemindersDraft {
    pub use_default: bool,
    #[serde(default)]
    pub overrides: Vec<crate::calendar::Reminder>,
}

/// An event's colour as the picker sets it (C13). Google's labels, which outrank a colour, aren't
/// PM's to set: an event with one keeps its colour as it is (the C13 spec's Adopted list, R-LABEL).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ColourDraft {
    /// No colour of its own: it shows in its calendar's colour.
    Calendar,
    /// One of Google's eleven event colours (`colorId` "1" to "11").
    Colour { id: String },
}

impl EventPatchDraft {
    /// The guest changes it makes: none when untouched, or touched and empty.
    pub fn guest_ops(&self) -> &[GuestOp] {
        self.guests.as_deref().unwrap_or_default()
    }

    /// Whether it changes the guest list or what guests may do (C12).
    pub fn touches_guests(&self) -> bool {
        !self.guest_ops().is_empty()
            || self.guests_can_modify.is_some()
            || self.guests_can_invite_others.is_some()
            || self.guests_can_see_other_guests.is_some()
    }

    /// Whether it changes anything about the event itself, beside the calendar it's on (C18d): a
    /// Save of the calendar alone sends no PATCH. Every field but the calendar counts, a field added
    /// later included; an empty guest list is no change, as everywhere, and neither is "Replace with
    /// plain text…" on its own, which only lets a description it sends through (C19).
    pub fn names_fields(&self) -> bool {
        let fields = EventPatchDraft {
            calendar: None,
            meet_request: None,
            replace_formatting: false,
            guests: self.guests.clone().filter(|ops| !ops.is_empty()),
            ..self.clone()
        };
        fields != EventPatchDraft::default()
    }
}

/// What the user wants for one guest after the save: on the event, required or optional, or off it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GuestState {
    Absent,
    Required,
    Optional,
}

/// One guest the user added, took off or marked optional: a change to Google's own guest list (R1),
/// never the list itself. Matched by address, whatever its case.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestOp {
    pub email: String,
    pub to: GuestState,
}

/// Who Google would email about a change if the user chose Send, so the choice can say so (R9, A3).
/// Never the addresses: the editor has them, and the question says only how many. The user and
/// rooms are never among them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GuestNotice {
    /// Guests already on the event who'd hear about the change.
    pub updated: u32,
    /// Guests new to the event: each gets an invitation.
    pub invited: u32,
    /// Guests the change takes off: each is told.
    pub removed: u32,
    /// Guests on an occurrence the change cancels and on nothing else it writes: each is told that
    /// occurrence is cancelled, not of an update. Never among `updated`; none when `cancelled`.
    pub told_cancelled: u32,
    /// The change deletes it (or every occurrence it reaches): `updated` are told it's cancelled.
    pub cancelled: bool,
    /// Different guests who'd hear, every write together.
    pub people: u32,
    /// Emails in all with Send: one per guest per event written. More than `people` when a change
    /// writes several events (a "this and following" change: the new part, the earlier part, each
    /// occurrence moved or cancelled).
    pub emails: u32,
    /// PM saw every guest list it counted. `false`: Google cut one short, or PM couldn't see every
    /// occurrence the change reaches: "at least".
    pub complete: bool,
}

impl GuestNotice {
    /// Whether an answer given to `asked` (the numbers the question showed) answers this: the same
    /// people, emails and completeness (C12 critiques C1-M5, C2-H3). Someone added meanwhile is
    /// asked about first, never emailed unseen.
    pub fn answered_by(&self, asked: &GuestNotice) -> bool {
        (self.people, self.emails, self.complete) == (asked.people, asked.emails, asked.complete)
    }
}

/// One guest as the editor shows it, from Google's fresh copy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GuestForEdit {
    pub email: String,
    pub name: Option<String>,
    /// As Google says it: accepted | declined | tentative | needsAction.
    pub response: Option<String>,
    pub optional: bool,
    pub organizer: bool,
    /// The calendar this copy is on (Google's `self`).
    #[serde(rename = "self")]
    pub is_self: bool,
    /// A room or another bookable thing (Google's `resource`, or its address).
    pub room: bool,
}

/// A change to how an event repeats (C11): a rule from the editor's set, or none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RepeatDraft {
    /// Repeat by this rule from now on.
    Rule { rule: RepeatRule },
    /// Stop repeating ("Does not repeat").
    Once,
}

/// A guest on a new event (C12d): who, and whether they're optional.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct NewGuest {
    pub email: String,
    #[serde(default)]
    pub optional: bool,
}

/// A new event as the editor holds it (A22: defined with its first caller, the create command).
/// Empty text is left out of the insert, never sent as "".
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct NewEventDraft {
    #[serde(default)]
    pub summary: String,
    pub time: TimeDraft,
    #[serde(default)]
    pub location: String,
    /// Plain lines, or formatting as PM's editor writes it.
    #[serde(default)]
    pub description: String,
    /// `None` is Google's default (busy).
    #[serde(default)]
    pub show_as: Option<ShowAs>,
    /// `None` or `Default` is the calendar's default, and isn't sent.
    #[serde(default)]
    pub visibility: Option<Visibility>,
    /// How it repeats; `None` doesn't repeat. Started on the first day the rule makes
    /// ([`crate::calendar_recur::expand::sync_start`]), its end moved with it (C11).
    #[serde(default)]
    pub recurrence: Option<RepeatRule>,
    /// Who it invites, in the order added (C12d): each gets an invitation if the user chooses Send.
    #[serde(default)]
    pub guests: Vec<NewGuest>,
    /// Google's `guestsCanModify`, `guestsCanInviteOthers` and `guestsCanSeeOtherGuests`: `None`, or
    /// Google's own default, isn't sent.
    #[serde(default)]
    pub guests_can_modify: Option<bool>,
    #[serde(default)]
    pub guests_can_invite_others: Option<bool>,
    #[serde(default)]
    pub guests_can_see_other_guests: Option<bool>,
    /// Its notifications (C13). `None`, or the calendar's defaults, sends nothing: Google gives a
    /// new event the calendar's own (INFERRED, probe E9).
    #[serde(default)]
    pub reminders: Option<RemindersDraft>,
    /// Its colour (C13). `None`, or the calendar's, sends nothing.
    #[serde(default)]
    pub colour: Option<ColourDraft>,
    /// Ask Google for a new Google Meet call (C18b), made once even if the create is sent again.
    #[serde(default)]
    pub meet: bool,
    /// The id of that request (`createRequest.requestId`): minted by the create command for each
    /// send, never by the webview (R5). A retry resends its first send's plan, which holds the first
    /// id, so Google makes one call however often the create goes.
    #[serde(skip)]
    pub meet_request: Option<String>,
}

/// What the user saw of an event when they chose to delete it, taken from the mirror row they
/// clicked. A delete sends no fields, so this is what decides whether a version Google changed in the
/// meantime may still be deleted ([`super::patch::delete_still_matches`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeenSummary {
    pub summary: String,
    /// As the mirror stores it: a date for an all-day event, else a UTC `…Z` instant.
    pub start: String,
    pub end: Option<String>,
    pub all_day: bool,
    pub location: Option<String>,
}

/// What the user saw of an event's time when they dragged it: the mirror row's start, end and kind,
/// in the mirror's own terms. A drag sends only times, so only times decide whether a version Google
/// changed since may still be moved ([`super::patch::timing_still_matches`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeenTimes {
    /// As the mirror stores it: a date for an all-day event, else a UTC `…Z` instant.
    pub start: String,
    pub end: Option<String>,
    pub all_day: bool,
}

/// Where a drag put an event, in this computer's wall time (the request's `device_zone`). The
/// backend says it again in the event's own zone, so a drag never changes which zone an event is in
/// ([`super::time::retime_draft`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MoveTo {
    Timed {
        start_date: String,
        start_time: String,
        end_date: String,
        end_time: String,
    },
    /// `last_day` is the last day covered, as [`TimeDraft::AllDay`] holds it.
    AllDay { first_day: String, last_day: String },
}

/// A drag of one event, as the webview hands it over: the mirror row, the time the user saw on it,
/// where it was dropped, and this computer's zone (`None` when the webview can't tell; never a
/// guessed UTC, R6). The choices and the move send the same values, which the choice token binds
/// (C16 critique C1-M4).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct MoveRequest {
    pub event_id: String,
    pub seen: SeenTimes,
    pub to: MoveTo,
    pub device_zone: Option<String>,
}

/// An Undo PM can offer for a write that just landed: the token that names it, and for how long.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UndoOffer {
    /// PM's own (a random id); never an event's id or an etag (R5).
    pub undo_token: String,
    /// How long the Undo is offered, in seconds: 8, as a delete's (A15).
    pub undo_seconds: u64,
}

/// Which occurrences of a repeating event a save or a delete is for: the dialog's three rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeriesScope {
    This,
    Following,
    All,
}

/// A reply to an invitation, in the reply control's words. Google's are `accepted`, `declined`,
/// `tentative`; "no reply yet" (`needsAction`) is never sent: Google web has no way back to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rsvp {
    Yes,
    No,
    Maybe,
}

impl Rsvp {
    /// Google's word for it.
    pub fn as_google(self) -> &'static str {
        match self {
            Rsvp::Yes => "accepted",
            Rsvp::No => "declined",
            Rsvp::Maybe => "tentative",
        }
    }
}

/// Which occurrences of a repeating event a reply is for. There is no "this and following": a guest
/// can't split someone else's repeating event (plan CUT), so it can't even be named.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RsvpScope {
    This,
    All,
}

/// The user's own reply on the copy the editor opened (the guest-list entry Google marks `self`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ReplyForEdit {
    /// As Google says it: accepted | declined | tentative | needsAction.
    pub response: Option<String>,
    /// Their note, "" for none. Everyone invited can see it.
    pub note: String,
}

/// What the calendar view may offer on one row, from the mirror's copy, assuming editing is on for
/// its account (the view merges each account's real status from the overview).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct ListRights {
    /// Why nothing in the editor may change, or `None` when something can.
    pub edit_block: Option<ReadOnlyReason>,
    /// Delete: the gate's own answer, so the list never offers one the send would refuse.
    pub can_delete: bool,
    /// Yes, No or Maybe.
    pub can_respond: bool,
    /// Its start and end: the gate's own answer, so the calendar never offers a drag the move would
    /// refuse (C16 critique C2-M4).
    pub can_retime: bool,
}

/// Why PM won't change an event, or one part of it. Each has a reason line in the editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadOnlyReason {
    /// An iCal subscription or an Outlook calendar.
    NotGoogle,
    /// Editing isn't on for the account (off, paused, or Google's permission is gone).
    EditingOff,
    /// The calendar is shared with you to view only, or PM doesn't know its role.
    CalendarReadOnly,
    /// You can edit this shared calendar's events except private ones, and this one is private.
    PrivateEvent,
    /// Someone else organises it and hasn't let guests change it: its shared fields stay theirs
    /// (C14).
    NotOrganizer,
    /// The calendar this copy is on has no entry on the event's guest list, so it has no reply to
    /// send (C14).
    NotInvited,
    /// The guest list of someone else's event stays the organiser's, even where they let guests
    /// change the rest (C14).
    OrganizerOnlyGuests,
    /// A one-off in PM that has since become a repeating event in Google: open one of its occurrences.
    BecameRepeating,
    /// Focus time, out of office, a working location, or a kind of Google event PM doesn't know:
    /// nothing may change (C21 or a card, Q8).
    SpecialType,
    /// A birthday or other special date: Google lets its name, notifications and colour change,
    /// and its date when it was added in Google Calendar. Nothing else (C20).
    SpecialDateFields,
    /// A special date linked to a contact: Google Contacts owns its day (C20).
    DateFromContacts,
    /// The calendar owner's own birthday: their Google Account owns its day (C20).
    DateFromProfile,
    /// Gmail added it: Google lets its busy or free, visibility, notifications, colour and the
    /// user's reply change. Nothing else (C20).
    FromGmailFields,
    /// Google has locked its title, time, place and description.
    Locked,
    /// The description has formatting PM can't edit, such as a table or coloured text
    /// ([`super::description::pm_can_edit`]): it stays as Google holds it unless the user replaces it
    /// with plain text (C19).
    UnsupportedFormatting,
    /// The event has a Google Calendar label, which sets its colour, and PM doesn't change labels:
    /// its colour stays as it is (R-LABEL, C13). Its notifications may still change.
    LabelReadOnly,
    /// Google holds a notification PM can't read, so a save that sends the list whole would lose
    /// it: the notifications stay as they are (C13).
    RemindersUnreadable,
    /// The event's video call is another service's, or a kind PM doesn't make, which PM shows and
    /// keeps (C18). A reason for the call alone ([`super::gate::reason_for`]).
    OtherCall,
}

/// What may change on one event, field by field, and why the rest can't. The backend is the
/// authority: the editor greys out what this says, and every save checks it again on a fresh copy.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct FieldPermissions {
    pub summary: bool,
    pub time: bool,
    /// How it repeats: closed with the time (Google locks the rule with the start), and on every
    /// event someone else organises, whatever they let guests change (C14 critiques C1-H2, C2-H2).
    pub repeat: bool,
    pub location: bool,
    pub description: bool,
    pub show_as: bool,
    pub visibility: bool,
    /// The guest list and what guests may do (C12).
    pub guests: bool,
    /// Its notifications and its colour (C13).
    pub reminders: bool,
    pub colour: bool,
    /// May add or remove a Google Meet call (C18).
    pub meet: bool,
    /// May go to another calendar of the same account (C18d): the organiser's to change. The save
    /// checks the calendar chosen.
    pub calendar: bool,
    pub delete: bool,
    /// The user may reply (Yes, No, Maybe). Not part of [`Self::any`]: a reply isn't one of the
    /// editor's fields (C14).
    pub respond: bool,
    /// The time may change only to one whole day, all day (Google: a birthday's "new values need
    /// to define an all-day event which spans exactly one day", C20). Not part of [`Self::any`]:
    /// it narrows `time`, it isn't a field.
    pub one_day: bool,
    /// Every reason something is read-only, most important first. Empty when everything may change.
    pub reasons: Vec<ReadOnlyReason>,
}

impl FieldPermissions {
    /// Nothing may change, for `reason`.
    pub fn none(reason: ReadOnlyReason) -> Self {
        FieldPermissions {
            reasons: vec![reason],
            ..Default::default()
        }
    }

    /// Whether anything in the editor may change. A reply alone isn't (C14).
    pub fn any(&self) -> bool {
        self.summary
            || self.time
            || self.repeat
            || self.location
            || self.description
            || self.show_as
            || self.visibility
            || self.guests
            || self.reminders
            || self.colour
            || self.meet
            || self.calendar
            || self.delete
    }

    /// The rights with a description PM can't edit opened again, for a save the user chose to make
    /// plain (C19). Only that reason reopens it: a locked description, or one the organiser keeps,
    /// stays closed, because the gate adds this reason only to a description that was otherwise
    /// open.
    pub fn replacing_formatting(mut self) -> Self {
        if self
            .reasons
            .contains(&ReadOnlyReason::UnsupportedFormatting)
        {
            self.description = true;
            // PM's own list of reasons, never fetched data.
            self.reasons
                .retain(|r| *r != ReadOnlyReason::UnsupportedFormatting);
        }
        self
    }
}

/// What a save came to, as data. `Saved` and `NoChange` are the successes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum WriteOutcome {
    Saved {
        warnings: Vec<WriteWarning>,
        /// Occurrences changed on their own that the write didn't reach (`YYYY-MM-DD`, in the
        /// series' zone): ones PM couldn't cancel, which may still show (`LeftOnOldSeries`), or, for
        /// a reply to every occurrence, ones answered on their own that kept their answer
        /// (`KeptOwnReply`, C14). Empty, and left out, for every other save.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        left_behind: Vec<String>,
        /// A "this and following" change that landed, with milestones linked to the earlier part
        /// dated on or after the day it starts, except one on an occurrence it left there: the
        /// webview asks about each (DECISIONS 17-10 #2).
        /// `None`, and left out, for every other save, and for a split with nothing to offer. Boxed,
        /// so every outcome (and every `Result` that carries one as its error) stays small.
        #[serde(skip_serializing_if = "Option::is_none")]
        milestone_moves: Option<Box<MilestoneMoves>>,
        /// Undo for a few seconds: a move or an editor's save of a one-off, or of "This event" of a
        /// repeating one (C16d), that Google answered with nothing to say beside it and that changed
        /// only what an Undo may put back ([`super::patch::UNDOABLE`]: the core fields, the
        /// notifications and the colour). `None`, and left out, for every other write.
        #[serde(skip_serializing_if = "Option::is_none")]
        undo: Option<UndoOffer>,
        /// PM's id of the calendar the event is on now, set only when the save put it on another
        /// calendar (C18d), so the notice names where it went from what Google holds, never from
        /// what the editor chose. `None`, and left out, for every other write.
        #[serde(skip_serializing_if = "Option::is_none")]
        calendar: Option<String>,
    },
    /// Google already holds exactly this; nothing was sent.
    NoChange,
    /// The event changed in Google since the editor opened, on a field this save touches (or, for a
    /// delete, on what the user saw). Nothing was overwritten; the draft is kept.
    Conflict {
        fields: Vec<String>,
    },
    /// The event is gone from Google (deleted, or cancelled from its series).
    Gone,
    ReadOnly {
        reason: ReadOnlyReason,
    },
    /// Google is rate-limiting; try again shortly. Nothing was saved.
    Busy,
    /// Google's sign-in for the account needs renewing.
    Reauth,
    /// The answer never arrived and PM couldn't look afterwards: it may or may not have happened.
    Unconfirmed,
    /// A new event whose answer never arrived, and PM couldn't tell whether Google made it. Calling
    /// the create command again with `retry_token` looks for it first and sends it again, under the
    /// same id, only if Google still doesn't show it (R2), so a second copy is very unlikely. PM's
    /// own token, never the event's id (R5); it lapses after eight hours or a restart.
    Ambiguous {
        retry_token: String,
    },
    /// Anything else, with Google's message clipped to plain text.
    Failed {
        message: String,
    },
    /// Something must be chosen first, and nothing was sent. `scope`: which occurrences (none was
    /// named, or the plan for those chosen isn't the one the dialog showed any more; R9). `guests`:
    /// whether Google emails these guests, Send or Don't send, which has no default (R9, C12).
    /// `again`: an answer came, given for other numbers than these (C12 critiques C1-M5, C2-H3).
    NeedsChoice {
        scope: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        guests: Option<GuestNotice>,
        #[serde(skip_serializing_if = "std::ops::Not::not")]
        again: bool,
    },
    /// Google answered as if it had saved the change, but kept the event as it was. `split`: the new
    /// part a "this and following" change made was removed too.
    IgnoredByGoogle {
        split: bool,
    },
    /// A "this and following" change PM couldn't finish: Google may hold part of it. `token` names
    /// it for settling; `offers` is what Google's state allows now; `from` is the day the change
    /// starts (`YYYY-MM-DD`, in the series' zone). Never an id or an etag (R5). `note`: a settle
    /// that didn't end the change says why here, while PM still holds it to settle (so its buttons
    /// stay); left out for every other answer. `emails`: the offers that would email guests, since
    /// the change went with Send (C12); left out when none would.
    PartialSplit {
        token: String,
        offers: Vec<SplitAction>,
        from: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        emails: Vec<SplitAction>,
    },
    /// The new part of a change PM couldn't finish was removed: the repeating event is as it was.
    SplitRemoved,
    /// The other changes in this save landed, and the calendar change didn't, for `message` (C18d).
    /// The editor reads Google's copy again before anything more is sent.
    NotMoved {
        message: String,
    },
}

impl WriteOutcome {
    /// Choose the occurrences (again).
    pub fn needs_scope() -> WriteOutcome {
        WriteOutcome::NeedsChoice {
            scope: true,
            guests: None,
            again: false,
        }
    }

    /// Choose whether Google emails these guests.
    pub fn needs_notify(guests: GuestNotice) -> WriteOutcome {
        WriteOutcome::NeedsChoice {
            scope: false,
            guests: Some(guests),
            again: false,
        }
    }

    /// Choose again whether Google emails these guests: the answer that came was for others.
    pub fn needs_notify_again(guests: GuestNotice) -> WriteOutcome {
        WriteOutcome::NeedsChoice {
            scope: false,
            guests: Some(guests),
            again: true,
        }
    }
}

/// What to do about a "this and following" change PM couldn't finish.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitAction {
    /// Delete the new part, once Google shows the original wasn't cut.
    Remove,
    /// Run the rest of the same change, once Google shows the part before it landed.
    Finish,
    /// Look again (reads only).
    Check,
}

/// What asking to delete came to: held for its Undo window, or refused at once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum DeleteStart {
    /// Waiting `undo_seconds` before it goes to Google; `undo_token` cancels it until then, and
    /// `calendar://delete-settled` reports how it ended.
    Held {
        undo_token: String,
        undo_seconds: u64,
        /// The mirror rows the delete hides while it waits: every occurrence it reaches, for "This
        /// and following" or "All events". Empty, and left out, for one event.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        hidden: Vec<String>,
    },
    /// Refused before anything was held (busy, read-only, editing off).
    Refused { result: WriteOutcome },
}

/// A delete still waiting out its Undo window, for a webview that reloaded and lost its own record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HeldDeleteInfo {
    pub undo_token: String,
    pub event_id: String,
    /// The title the user saw.
    pub summary: String,
    pub seconds_left: u64,
    /// The rows it hides, as [`DeleteStart::Held`] gave them.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hidden: Vec<String>,
}

/// The `calendar://delete-settled` payload: a held delete went to Google, and this is what came of
/// it. An undone delete sends none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DeleteSettled {
    pub undo_token: String,
    pub event_id: String,
    pub result: WriteOutcome,
}

/// Saved, but with something the user should know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteWarning {
    /// Google took the change, but the calendar couldn't be refreshed yet; the next sync will.
    MirrorRefreshPending,
    /// Some occurrences changed on their own couldn't be cancelled: `Saved.left_behind` says which.
    LeftOnOldSeries,
    /// Saved in Google, but it starts outside the months PM mirrors, so PM's calendar won't show it.
    OutsideMirror,
    /// A "this and following" change landed, and PM couldn't tell which repeating event Google made
    /// of the new part, so it can't offer to move the milestones `Saved.milestone_moves` names: they
    /// stay on the earlier part (C9b-2 critique C2-M1).
    MilestoneNotOffered,
    /// Google applied a one-event save without keeping all of it as sent (a colour it wouldn't clear
    /// beside a new title, say): what Google holds is in the mirror, and the words say to look (C13
    /// review round 1). Never "nothing changed", and never a plain "Saved".
    PartlyApplied,
    /// A reply to every occurrence landed, and these occurrences, answered on their own before,
    /// kept their own answer: `Saved.left_behind` says which (C14 critiques C1-M3, C2-H1).
    KeptOwnReply,
    /// Saved; Google is still making the video call asked for, and its link shows after a later
    /// refresh (C18).
    CallPending,
    /// Saved, but Google didn't make the video call asked for (C18).
    CallFailed,
    /// On its new calendar Google gave the event a new identity, so PM's links to it (a milestone, a
    /// reminder to prepare) stayed with the old one (C18d). Raised only when one does.
    LinksLeftBehind,
}

/// What moving milestones to the new part of a "this and following" change needs (C9b-2). Dates
/// are `YYYY-MM-DD` in the user's zone, as the milestone list resolves them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MilestoneMoves {
    /// The new part's iCal UID (Google mints it; the insert never sets one), where Move sends a
    /// milestone. `None`: PM couldn't tell, so nothing can be moved ([`WriteWarning::MilestoneNotOffered`]).
    pub new_series_ical: Option<String>,
    /// The earlier part's, where Move back sends it.
    pub earlier_series_ical: String,
    /// The new part's title, and the earlier part's, as Google holds them after the change.
    pub new_title: String,
    pub earlier_title: String,
    /// The new part's first occurrence, the date a moved milestone shows until the next read; `None`
    /// when PM can't say.
    pub first_day: Option<String>,
    /// The earlier part's last occurrence before the change: a milestone left on it reads overdue
    /// from then. `None` when PM can't say.
    pub earlier_last_day: Option<String>,
    /// Today, as the change landed: whether that last day has passed yet, for the words (C9b-2
    /// review round 3, R3-F3).
    pub today: String,
    /// Never empty.
    pub milestones: Vec<MilestoneOffer>,
}

/// A milestone PM may move: PM's own id, its words, and its date before the change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MilestoneOffer {
    pub id: i64,
    pub label: String,
    pub project: String,
    pub date: String,
}

/// One event as the editor shows it, read from a fresh copy (never the mirror, which clips text).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EventForEdit {
    pub summary: String,
    pub location: String,
    /// As Google holds it, plain lines or formatted: the webview reads which, and whether its
    /// editor writes it back; the gate says whether a save may change it
    /// ([`super::description::pm_can_edit`]).
    pub description: String,
    /// In the event's own zone(s), as the editor's fields hold them.
    pub time: TimeDraft,
    /// Google's exact start and end of a timed event (RFC 3339), `None` for an all-day one. `HH:MM`
    /// can't say which of a repeated hour's two 01:30s a half is; the save keeps Google's own instant
    /// for a half the user leaves alone, and the editor's pre-save check judges it by these.
    pub start_at: Option<String>,
    pub end_at: Option<String>,
    pub show_as: ShowAs,
    pub visibility: Visibility,
    /// Attachment titles, listed read-only; PM never sends the attachments field, so it keeps them.
    pub attachments: Vec<String>,
    /// Google's own page for the event.
    pub html_link: Option<String>,
    /// Every entry of Google's guest list with an address, the user's and the organiser's included,
    /// in Google's order (C12).
    pub guests: Vec<GuestForEdit>,
    /// What guests may do, Google's defaults filled in: modify (false), invite others (true), see
    /// the guest list (true).
    pub guests_can_modify: bool,
    pub guests_can_invite_others: bool,
    pub guests_can_see_other_guests: bool,
    /// Google cut the guest list short (`attendeesOmitted`), so it can't be changed.
    pub guests_omitted: bool,
    /// The organiser's address (Google's `organizer.email`), on the guest list or not: no guest
    /// change may touch it (`attendees::check`), so the editor never makes one.
    pub organizer_email: Option<String>,
    /// Its notifications as Google holds them; none at all reads as the calendar's defaults (C13).
    /// A list PM can't read is shown as the defaults only while the permission keeps it read-only.
    pub reminders: RemindersDraft,
    /// Google's `colorId` and `eventLabelId`, `None` when unset. A label, which PM doesn't change,
    /// sets the colour Google draws (C13).
    pub color_id: Option<String>,
    pub label_id: Option<String>,
    /// The user's own reply, from the entry Google marks as the calendar this copy is on; `None`
    /// without one (C14).
    pub reply: Option<ReplyForEdit>,
    /// Google's `organizer.self`: the calendar this copy is on organises the event.
    pub organizer_self: bool,
    /// Its video call as Google holds it, `None` for none (C18).
    pub call: Option<CallForEdit>,
    /// The event's calendar offers Google Meet (C18).
    pub meet_offered: bool,
}

/// What opening the editor came to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum EditLoad {
    /// `session` names this editing session in later saves: the backend keeps the copy it was opened
    /// on, so the webview never holds an etag or Google's raw event.
    Ready {
        session: String,
        event: Box<EventForEdit>,
        permissions: FieldPermissions,
        /// What the editor showed, in the mirror's terms, for a delete started from it: the
        /// fresh copy, not a mirror row that may be older. `None` if it can't be put in those terms.
        seen: Option<SeenSummary>,
        /// Its series, when it's an occurrence of one and PM could read Google's copy of it. The
        /// session keeps that copy, so "all events" and "this and following" never overwrite a
        /// change someone made to the series after the editor opened (G9). Boxed, as its rule (C11)
        /// makes it the outcome's largest part.
        series: Option<Box<SeriesInfo>>,
    },
    Gone,
    Reauth,
    Failed {
        message: String,
    },
}

/// An occurrence's series, as the editor shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SeriesInfo {
    /// It's the series' first occurrence, where "this and following" is every occurrence.
    pub first: bool,
    /// How the series repeats, as Google Calendar says it ("Weekly on Monday").
    pub repeats: String,
    /// The rule as one from the editor's set, or `None` for one PM can only show (`repeats` says
    /// it), which can only be replaced (C11).
    pub rule: Option<RepeatRule>,
    /// The series' first occurrence's day, `YYYY-MM-DD`, in the zone it repeats in.
    pub starts: String,
    /// The zone it repeats in; `None` for an all-day one, which repeats by days.
    pub zone: Option<String>,
}

/// What the choices for a change to a repeating occurrence are asked for: an open editor's draft, a
/// delete of a mirror row (`seen` is what the user saw of it), or a drag of one (C16).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChoiceRequest {
    Edit {
        session: String,
        /// Boxed, as its notifications and colour (C13) make it the request's largest part.
        draft: Box<EventPatchDraft>,
    },
    Delete {
        event_id: String,
        seen: SeenSummary,
    },
    /// A drag of an occurrence: the row dragged, what the user saw of its time, and where it went.
    Move(MoveRequest),
}

/// "This event", "This and following" and "All events" for one change, each planned against Google's
/// copies as they are now (DS12): what it would do, or why it can't.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum SeriesChoices {
    /// A one-off event: no choice to make.
    NotRepeating,
    Ready {
        this: ScopeRow,
        following: ScopeRow,
        all: ScopeRow,
        /// Something to say once about this series, beside the rows: how a change PM was making
        /// when it closed came out.
        #[serde(skip_serializing_if = "Option::is_none")]
        notice: Option<String>,
    },
    /// Nothing can be offered: the event is gone, read-only, needs a new sign-in, or Google couldn't
    /// be asked.
    Refused { result: WriteOutcome },
}

/// One of the three choices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "row", rename_all = "snake_case")]
pub enum ScopeRow {
    /// It can go ahead. `token` is what the save names to say the user chose this plan; a plan that
    /// changed since gets asked about again. `guests`: who Google would email with Send, left out
    /// when nobody (C12).
    Offered {
        warnings: Vec<SeriesWarningDto>,
        token: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        guests: Option<GuestNotice>,
    },
    /// "This and following" from the series' first occurrence: every occurrence, as "All events".
    AsAll {
        warnings: Vec<SeriesWarningDto>,
        token: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        guests: Option<GuestNotice>,
    },
    /// Not for this change, and why, in words.
    Refused { message: String },
    /// Changed in Google since the editor opened, on these fields.
    Conflict { fields: Vec<String> },
    /// A change PM couldn't finish holds this series (C9 design review M1): settle it first, as
    /// [`WriteOutcome::PartialSplit`] says, `emails` included.
    Pending {
        token: String,
        offers: Vec<SplitAction>,
        from: String,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        emails: Vec<SplitAction>,
    },
}

/// What the user should know before choosing a row. Dates are `YYYY-MM-DD` in the series' zone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "warning", rename_all = "snake_case")]
pub enum SeriesWarningDto {
    /// Another part of this series was split off earlier; Google may treat it as one with this.
    SplitSibling,
    /// This many occurrences were changed on their own; Google may keep or reset their changes.
    /// `complete` false: PM couldn't see every one, so it's "some".
    ExceptionsMayReset { count: u32, complete: bool },
    /// The series' first occurrence moves.
    StartMoved { from: String, to: String },
    /// The days the series falls on move, as this occurrence moves from `from` to `to`, by
    /// `shift_days`. `before` and `after` are those days in words ("every Sunday"), or `None` where
    /// the rule names no day (a daily series).
    RepeatDayChanges {
        from: String,
        to: String,
        before: Option<String>,
        after: Option<String>,
        shift_days: i64,
    },
    /// Every occurrence gets this one's length.
    LengthChanges,
    /// The new part of a split doesn't take the series' attachments, video call or label.
    NotCarried {
        attachments: bool,
        conference: bool,
        label: bool,
    },
    /// The new part is a new event in Google: links to the old one stay with the part before.
    NewIdentity,
    /// It stops repeating: only the occurrence on `day` stays (C11).
    BecomesOneEvent { day: String },
    /// This many occurrences changed on their own fall on days the new rule skips: their changes go.
    ChangesLost { count: u32 },
    /// Every occurrence goes to the calendar PM calls `to` (C18e).
    ChangesCalendar { to: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The webview matches these exact strings.
    #[test]
    fn outcomes_and_reasons_serialise_to_the_strings_the_editor_names() {
        let json = serde_json::to_string(&WriteOutcome::Conflict {
            fields: vec!["summary".into()],
        })
        .unwrap();
        assert_eq!(json, r#"{"outcome":"conflict","fields":["summary"]}"#);
        assert_eq!(
            serde_json::to_string(&WriteOutcome::ReadOnly {
                reason: ReadOnlyReason::UnsupportedFormatting
            })
            .unwrap(),
            r#"{"outcome":"read_only","reason":"unsupported_formatting"}"#
        );
        assert_eq!(
            serde_json::to_string(&WriteOutcome::ReadOnly {
                reason: ReadOnlyReason::BecameRepeating
            })
            .unwrap(),
            r#"{"outcome":"read_only","reason":"became_repeating"}"#
        );
        // C13: a label holding the colour, and notifications PM can't read.
        for (reason, json) in [
            (
                ReadOnlyReason::LabelReadOnly,
                r#"{"outcome":"read_only","reason":"label_read_only"}"#,
            ),
            (
                ReadOnlyReason::RemindersUnreadable,
                r#"{"outcome":"read_only","reason":"reminders_unreadable"}"#,
            ),
            // C14: a copy whose calendar isn't on the guest list has no reply to send.
            (
                ReadOnlyReason::NotInvited,
                r#"{"outcome":"read_only","reason":"not_invited"}"#,
            ),
            // C14c: the guest list of someone else's event stays the organiser's.
            (
                ReadOnlyReason::OrganizerOnlyGuests,
                r#"{"outcome":"read_only","reason":"organizer_only_guests"}"#,
            ),
            // C18: another service's video call, shown and kept.
            (
                ReadOnlyReason::OtherCall,
                r#"{"outcome":"read_only","reason":"other_call"}"#,
            ),
            // C20: a birthday's and a Gmail event's own lists, and who owns a birthday's date.
            (
                ReadOnlyReason::SpecialDateFields,
                r#"{"outcome":"read_only","reason":"special_date_fields"}"#,
            ),
            (
                ReadOnlyReason::DateFromContacts,
                r#"{"outcome":"read_only","reason":"date_from_contacts"}"#,
            ),
            (
                ReadOnlyReason::DateFromProfile,
                r#"{"outcome":"read_only","reason":"date_from_profile"}"#,
            ),
            (
                ReadOnlyReason::FromGmailFields,
                r#"{"outcome":"read_only","reason":"from_gmail_fields"}"#,
            ),
        ] {
            assert_eq!(
                serde_json::to_string(&WriteOutcome::ReadOnly { reason }).unwrap(),
                json
            );
        }
        assert_eq!(
            serde_json::to_string(&WriteOutcome::Saved {
                warnings: vec![WriteWarning::MirrorRefreshPending],
                left_behind: Vec::new(),
                milestone_moves: None,
                undo: None,
                calendar: None,
            })
            .unwrap(),
            r#"{"outcome":"saved","warnings":["mirror_refresh_pending"]}"#
        );
        // C18d: a save that put the event on another calendar names it, by PM's id.
        assert_eq!(
            serde_json::to_string(&WriteOutcome::Saved {
                warnings: Vec::new(),
                left_behind: Vec::new(),
                milestone_moves: None,
                undo: None,
                calendar: Some("cal-b".into()),
            })
            .unwrap(),
            r#"{"outcome":"saved","warnings":[],"calendar":"cal-b"}"#
        );
        // The webview's `WriteWarning` names it so (C13 review round 1), the video call's two
        // (C18), and links left with the old identity (C18d).
        for (warning, json) in [
            (WriteWarning::PartlyApplied, r#""partly_applied""#),
            (WriteWarning::CallPending, r#""call_pending""#),
            (WriteWarning::CallFailed, r#""call_failed""#),
            (WriteWarning::LinksLeftBehind, r#""links_left_behind""#),
        ] {
            assert_eq!(serde_json::to_string(&warning).unwrap(), json);
        }
        assert_eq!(
            serde_json::to_string(&WriteOutcome::Saved {
                warnings: vec![WriteWarning::LeftOnOldSeries],
                left_behind: vec!["2026-11-02".into()],
                milestone_moves: None,
                undo: None,
                calendar: None,
            })
            .unwrap(),
            r#"{"outcome":"saved","warnings":["left_on_old_series"],"left_behind":["2026-11-02"]}"#
        );
        // C14: a reply to every occurrence that some occurrences answered on their own kept.
        assert_eq!(
            serde_json::to_string(&WriteOutcome::Saved {
                warnings: vec![WriteWarning::KeptOwnReply],
                left_behind: vec!["2026-11-02".into()],
                milestone_moves: None,
                undo: None,
                calendar: None,
            })
            .unwrap(),
            r#"{"outcome":"saved","warnings":["kept_own_reply"],"left_behind":["2026-11-02"]}"#
        );
        for (outcome, json) in [
            (
                WriteOutcome::needs_scope(),
                r#"{"outcome":"needs_choice","scope":true}"#,
            ),
            (
                WriteOutcome::needs_notify(GuestNotice {
                    updated: 2,
                    people: 2,
                    emails: 2,
                    complete: true,
                    ..Default::default()
                }),
                r#"{"outcome":"needs_choice","scope":false,"guests":{"updated":2,"invited":0,"removed":0,"told_cancelled":0,"cancelled":false,"people":2,"emails":2,"complete":true}}"#,
            ),
            (
                WriteOutcome::needs_notify_again(GuestNotice {
                    invited: 1,
                    people: 1,
                    emails: 1,
                    ..Default::default()
                }),
                r#"{"outcome":"needs_choice","scope":false,"guests":{"updated":0,"invited":1,"removed":0,"told_cancelled":0,"cancelled":false,"people":1,"emails":1,"complete":false},"again":true}"#,
            ),
            (
                WriteOutcome::IgnoredByGoogle { split: false },
                r#"{"outcome":"ignored_by_google","split":false}"#,
            ),
            (
                WriteOutcome::IgnoredByGoogle { split: true },
                r#"{"outcome":"ignored_by_google","split":true}"#,
            ),
            (
                WriteOutcome::PartialSplit {
                    token: "t".into(),
                    offers: vec![SplitAction::Remove, SplitAction::Check],
                    from: "2026-11-02".into(),
                    note: None,
                    emails: Vec::new(),
                },
                r#"{"outcome":"partial_split","token":"t","offers":["remove","check"],"from":"2026-11-02"}"#,
            ),
            (
                WriteOutcome::PartialSplit {
                    token: "t".into(),
                    offers: vec![SplitAction::Check],
                    from: "2026-11-02".into(),
                    note: Some("Try again in a moment.".into()),
                    emails: Vec::new(),
                },
                r#"{"outcome":"partial_split","token":"t","offers":["check"],"from":"2026-11-02","note":"Try again in a moment."}"#,
            ),
            // C12: sent with Send, Remove would email guests.
            (
                WriteOutcome::PartialSplit {
                    token: "t".into(),
                    offers: vec![SplitAction::Remove, SplitAction::Check],
                    from: "2026-11-02".into(),
                    note: None,
                    emails: vec![SplitAction::Remove],
                },
                r#"{"outcome":"partial_split","token":"t","offers":["remove","check"],"from":"2026-11-02","emails":["remove"]}"#,
            ),
            (WriteOutcome::SplitRemoved, r#"{"outcome":"split_removed"}"#),
            // C18d: the other changes saved, the calendar change not.
            (
                WriteOutcome::NotMoved {
                    message: "x".into(),
                },
                r#"{"outcome":"not_moved","message":"x"}"#,
            ),
            (WriteOutcome::NoChange, r#"{"outcome":"no_change"}"#),
            (WriteOutcome::Gone, r#"{"outcome":"gone"}"#),
            (WriteOutcome::Busy, r#"{"outcome":"busy"}"#),
            (WriteOutcome::Reauth, r#"{"outcome":"reauth"}"#),
            (WriteOutcome::Unconfirmed, r#"{"outcome":"unconfirmed"}"#),
            (
                WriteOutcome::Ambiguous {
                    retry_token: "ref-1".into(),
                },
                r#"{"outcome":"ambiguous","retry_token":"ref-1"}"#,
            ),
            (
                WriteOutcome::Saved {
                    warnings: vec![WriteWarning::OutsideMirror],
                    left_behind: Vec::new(),
                    milestone_moves: None,
                    undo: None,
                    calendar: None,
                },
                r#"{"outcome":"saved","warnings":["outside_mirror"]}"#,
            ),
            (
                WriteOutcome::Failed {
                    message: "Backend Error".into(),
                },
                r#"{"outcome":"failed","message":"Backend Error"}"#,
            ),
        ] {
            assert_eq!(serde_json::to_string(&outcome).unwrap(), json);
        }
        assert_eq!(Notify::ExternalOnly.as_param(), "externalOnly");
        assert_eq!(
            serde_json::to_string(&DeleteStart::Held {
                undo_token: "t".into(),
                undo_seconds: 8,
                hidden: Vec::new(),
            })
            .unwrap(),
            r#"{"outcome":"held","undo_token":"t","undo_seconds":8}"#
        );
        assert_eq!(
            serde_json::to_string(&DeleteStart::Held {
                undo_token: "t".into(),
                undo_seconds: 8,
                hidden: vec!["cal:a".into(), "cal:b".into()],
            })
            .unwrap(),
            r#"{"outcome":"held","undo_token":"t","undo_seconds":8,"hidden":["cal:a","cal:b"]}"#
        );
        for (scope, json) in [
            (SeriesScope::This, r#""this""#),
            (SeriesScope::Following, r#""following""#),
            (SeriesScope::All, r#""all""#),
        ] {
            assert_eq!(serde_json::from_str::<SeriesScope>(json).unwrap(), scope);
        }
        assert_eq!(
            serde_json::to_string(&DeleteStart::Refused {
                result: WriteOutcome::Busy
            })
            .unwrap(),
            r#"{"outcome":"refused","result":{"outcome":"busy"}}"#
        );
        assert_eq!(
            serde_json::to_string(&DeleteSettled {
                undo_token: "t".into(),
                event_id: "cal:e".into(),
                result: WriteOutcome::Gone
            })
            .unwrap(),
            r#"{"undo_token":"t","event_id":"cal:e","result":{"outcome":"gone"}}"#
        );
        let draft: TimeDraft = serde_json::from_str(
            r#"{"kind":"all_day","first_day":"2026-10-10","last_day":"2026-10-12"}"#,
        )
        .unwrap();
        assert!(matches!(draft, TimeDraft::AllDay { .. }));
    }

    /// C9b-2: a split's milestone offers, in the shape the webview reads; a new part PM couldn't
    /// name is `null` beside its warning.
    #[test]
    fn saved_with_milestone_moves_serialises_as_the_webview_reads_it() {
        let moves = MilestoneMoves {
            new_series_ical: Some("tail@google.com".into()),
            earlier_series_ical: "abc@google.com".into(),
            new_title: "Gym".into(),
            earlier_title: "Gym".into(),
            first_day: Some("2026-10-19".into()),
            earlier_last_day: Some("2026-10-12".into()),
            today: "2026-10-13".into(),
            milestones: vec![MilestoneOffer {
                id: 7,
                label: "Launch".into(),
                project: "Atlas".into(),
                date: "2026-10-19".into(),
            }],
        };
        assert_eq!(
            serde_json::to_string(&WriteOutcome::Saved {
                warnings: Vec::new(),
                left_behind: Vec::new(),
                milestone_moves: Some(Box::new(moves.clone())),
                undo: None,
                calendar: None,
            })
            .unwrap(),
            r#"{"outcome":"saved","warnings":[],"milestone_moves":{"new_series_ical":"tail@google.com","earlier_series_ical":"abc@google.com","new_title":"Gym","earlier_title":"Gym","first_day":"2026-10-19","earlier_last_day":"2026-10-12","today":"2026-10-13","milestones":[{"id":7,"label":"Launch","project":"Atlas","date":"2026-10-19"}]}}"#
        );
        assert_eq!(
            serde_json::to_string(&WriteOutcome::Saved {
                warnings: vec![WriteWarning::MilestoneNotOffered],
                left_behind: Vec::new(),
                milestone_moves: Some(Box::new(MilestoneMoves {
                    new_series_ical: None,
                    first_day: None,
                    earlier_last_day: None,
                    ..moves
                })),
                undo: None,
                calendar: None,
            })
            .unwrap(),
            r#"{"outcome":"saved","warnings":["milestone_not_offered"],"milestone_moves":{"new_series_ical":null,"earlier_series_ical":"abc@google.com","new_title":"Gym","earlier_title":"Gym","first_day":null,"earlier_last_day":null,"today":"2026-10-13","milestones":[{"id":7,"label":"Launch","project":"Atlas","date":"2026-10-19"}]}}"#
        );
    }

    /// C16a: a drop as the webview sends it (both kinds of time, a zone or none), a move's choices
    /// asked with the same fields, and `saved` with and without its Undo (left out when none).
    #[test]
    fn move_to_and_offers_read_and_serialise_as_the_view_names_them() {
        let timed: MoveRequest = serde_json::from_str(
            r#"{"event_id":"cal:e","seen":{"start":"2026-10-12T08:00:00Z",
                "end":"2026-10-12T09:00:00Z","all_day":false},"to":{"kind":"timed",
                "start_date":"2026-10-13","start_time":"09:30","end_date":"2026-10-13",
                "end_time":"10:30"},"device_zone":"Europe/London"}"#,
        )
        .unwrap();
        assert_eq!(
            timed,
            MoveRequest {
                event_id: "cal:e".into(),
                seen: SeenTimes {
                    start: "2026-10-12T08:00:00Z".into(),
                    end: Some("2026-10-12T09:00:00Z".into()),
                    all_day: false,
                },
                to: MoveTo::Timed {
                    start_date: "2026-10-13".into(),
                    start_time: "09:30".into(),
                    end_date: "2026-10-13".into(),
                    end_time: "10:30".into(),
                },
                device_zone: Some("Europe/London".into()),
            }
        );
        let all_day: MoveTo = serde_json::from_str(
            r#"{"kind":"all_day","first_day":"2026-10-13","last_day":"2026-10-14"}"#,
        )
        .unwrap();
        assert_eq!(
            all_day,
            MoveTo::AllDay {
                first_day: "2026-10-13".into(),
                last_day: "2026-10-14".into(),
            }
        );
        let request: ChoiceRequest = serde_json::from_str(
            r#"{"kind":"move","event_id":"cal:e","seen":{"start":"2026-10-12","end":"2026-10-13",
                "all_day":true},"to":{"kind":"all_day","first_day":"2026-10-14",
                "last_day":"2026-10-14"},"device_zone":null}"#,
        )
        .unwrap();
        assert!(matches!(
            &request,
            ChoiceRequest::Move(m) if m.event_id == "cal:e" && m.device_zone.is_none()
                && m.seen.all_day && m.seen.end.as_deref() == Some("2026-10-13")
        ));
        assert_eq!(
            serde_json::to_string(&WriteOutcome::Saved {
                warnings: Vec::new(),
                left_behind: Vec::new(),
                milestone_moves: None,
                undo: Some(UndoOffer {
                    undo_token: "u1".into(),
                    undo_seconds: 8,
                }),
                calendar: None,
            })
            .unwrap(),
            r#"{"outcome":"saved","warnings":[],"undo":{"undo_token":"u1","undo_seconds":8}}"#
        );
        assert_eq!(
            serde_json::to_string(&WriteOutcome::Saved {
                warnings: Vec::new(),
                left_behind: Vec::new(),
                milestone_moves: None,
                undo: None,
                calendar: None,
            })
            .unwrap(),
            r#"{"outcome":"saved","warnings":[]}"#
        );
    }

    /// The series choices, in the shapes the webview reads and sends.
    #[test]
    fn series_choices_serialise_to_the_strings_the_dialog_names() {
        let request: ChoiceRequest =
            serde_json::from_str(r#"{"kind":"edit","session":"s1","draft":{"summary":"Retro"}}"#)
                .unwrap();
        assert!(matches!(
            request,
            ChoiceRequest::Edit { session, draft } if session == "s1"
                && draft.summary.as_deref() == Some("Retro")
        ));
        let request: ChoiceRequest = serde_json::from_str(
            r#"{"kind":"delete","event_id":"cal:e","seen":{"summary":"Gym","start":"2026-10-19",
                "end":null,"all_day":true,"location":null}}"#,
        )
        .unwrap();
        assert!(matches!(request, ChoiceRequest::Delete { event_id, .. } if event_id == "cal:e"));
        let choices = SeriesChoices::Ready {
            this: ScopeRow::Offered {
                warnings: Vec::new(),
                token: "t1".into(),
                guests: None,
            },
            following: ScopeRow::AsAll {
                guests: None,
                warnings: vec![
                    SeriesWarningDto::SplitSibling,
                    SeriesWarningDto::ExceptionsMayReset {
                        count: 2,
                        complete: false,
                    },
                    SeriesWarningDto::RepeatDayChanges {
                        from: "2026-10-18".into(),
                        to: "2026-10-19".into(),
                        before: Some("every Sunday".into()),
                        after: None,
                        shift_days: 1,
                    },
                ],
                token: "t2".into(),
            },
            all: ScopeRow::Conflict {
                fields: vec!["summary".into()],
            },
            notice: None,
        };
        assert_eq!(
            serde_json::to_string(&choices).unwrap(),
            r#"{"outcome":"ready","this":{"row":"offered","warnings":[],"token":"t1"},"following":{"row":"as_all","warnings":[{"warning":"split_sibling"},{"warning":"exceptions_may_reset","count":2,"complete":false},{"warning":"repeat_day_changes","from":"2026-10-18","to":"2026-10-19","before":"every Sunday","after":null,"shift_days":1}],"token":"t2"},"all":{"row":"conflict","fields":["summary"]}}"#
        );
        let refused = |message: &str| ScopeRow::Refused {
            message: message.into(),
        };
        assert_eq!(
            serde_json::to_string(&SeriesChoices::Ready {
                this: refused("a"),
                following: refused("b"),
                all: refused("c"),
                notice: Some("Said once.".into()),
            })
            .unwrap(),
            r#"{"outcome":"ready","this":{"row":"refused","message":"a"},"following":{"row":"refused","message":"b"},"all":{"row":"refused","message":"c"},"notice":"Said once."}"#
        );
        assert_eq!(
            serde_json::to_string(&ScopeRow::Pending {
                token: "t".into(),
                offers: vec![SplitAction::Finish],
                from: "2026-11-02".into(),
                emails: Vec::new(),
            })
            .unwrap(),
            r#"{"row":"pending","token":"t","offers":["finish"],"from":"2026-11-02"}"#
        );
        assert_eq!(
            serde_json::to_string(&ScopeRow::Pending {
                token: "t".into(),
                offers: vec![SplitAction::Finish],
                from: "2026-11-02".into(),
                emails: vec![SplitAction::Finish],
            })
            .unwrap(),
            r#"{"row":"pending","token":"t","offers":["finish"],"from":"2026-11-02","emails":["finish"]}"#
        );
        // C12: a row that would email someone says who, in numbers.
        assert_eq!(
            serde_json::to_string(&ScopeRow::Offered {
                warnings: Vec::new(),
                token: "t".into(),
                guests: Some(GuestNotice {
                    invited: 1,
                    people: 1,
                    emails: 1,
                    complete: true,
                    ..Default::default()
                }),
            })
            .unwrap(),
            r#"{"row":"offered","warnings":[],"token":"t","guests":{"updated":0,"invited":1,"removed":0,"told_cancelled":0,"cancelled":false,"people":1,"emails":1,"complete":true}}"#
        );
        for (text, action) in [
            (r#""remove""#, SplitAction::Remove),
            (r#""finish""#, SplitAction::Finish),
            (r#""check""#, SplitAction::Check),
        ] {
            assert_eq!(serde_json::from_str::<SplitAction>(text).unwrap(), action);
        }
        for (choices, json) in [
            (
                SeriesChoices::NotRepeating,
                r#"{"outcome":"not_repeating"}"#,
            ),
            (
                SeriesChoices::Refused {
                    result: WriteOutcome::Reauth,
                },
                r#"{"outcome":"refused","result":{"outcome":"reauth"}}"#,
            ),
        ] {
            assert_eq!(serde_json::to_string(&choices).unwrap(), json);
        }
        assert_eq!(
            serde_json::to_string(&ScopeRow::Refused {
                message: "No.".into()
            })
            .unwrap(),
            r#"{"row":"refused","message":"No."}"#
        );
        assert_eq!(
            serde_json::to_string(&SeriesInfo {
                first: false,
                repeats: "Weekly on Monday".into(),
                rule: None,
                starts: "2026-10-05".into(),
                zone: Some("Europe/London".into()),
            })
            .unwrap(),
            r#"{"first":false,"repeats":"Weekly on Monday","rule":null,"starts":"2026-10-05","zone":"Europe/London"}"#
        );
        for (warning, json) in [
            (
                SeriesWarningDto::BecomesOneEvent {
                    day: "2026-10-05".into(),
                },
                r#"{"warning":"becomes_one_event","day":"2026-10-05"}"#,
            ),
            (
                SeriesWarningDto::ChangesLost { count: 2 },
                r#"{"warning":"changes_lost","count":2}"#,
            ),
            // C18e: where every occurrence goes.
            (
                SeriesWarningDto::ChangesCalendar { to: "Work".into() },
                r#"{"warning":"changes_calendar","to":"Work"}"#,
            ),
        ] {
            assert_eq!(serde_json::to_string(&warning).unwrap(), json);
        }
    }

    /// C11: a draft's repeat, as the editor will send it; left out or null, the repeat is untouched.
    #[test]
    fn a_draft_names_a_new_repeat_or_none() {
        let rule: EventPatchDraft = serde_json::from_str(
            r#"{"recurrence":{"kind":"rule","rule":{"pattern":{"kind":"daily"},"interval":2,
                "ends":{"kind":"never"}}}}"#,
        )
        .unwrap();
        assert!(matches!(
            rule.recurrence,
            Some(RepeatDraft::Rule { rule }) if rule.interval == 2 && rule.to_spec().is_ok()
        ));
        let once: EventPatchDraft =
            serde_json::from_str(r#"{"recurrence":{"kind":"once"}}"#).unwrap();
        assert_eq!(once.recurrence, Some(RepeatDraft::Once));
        for json in [r#"{"summary":"Gym"}"#, r#"{"recurrence":null}"#] {
            let untouched: EventPatchDraft = serde_json::from_str(json).unwrap();
            assert!(untouched.recurrence.is_none(), "{json}");
        }
        assert!(
            serde_json::from_str::<EventPatchDraft>(r#"{"recurrence":{"kind":"weekly"}}"#).is_err()
        );
    }

    /// C18d: a draft names the calendar to put the event on by PM's id, and only a field beside it
    /// is a field change: a Save of the calendar alone sends no PATCH.
    #[test]
    fn a_draft_names_a_calendar_and_counts_only_fields_beside_it() {
        let only: EventPatchDraft = serde_json::from_str(r#"{"calendar":"cal-b"}"#).unwrap();
        assert_eq!(only.calendar.as_deref(), Some("cal-b"));
        assert!(!only.names_fields());
        let untouched: EventPatchDraft = serde_json::from_str(r#"{"summary":"Gym"}"#).unwrap();
        assert!(untouched.calendar.is_none());
        assert!(untouched.names_fields());
        // An empty guest list is no change; a call's request id alone is none either (the backend's).
        let empty = EventPatchDraft {
            calendar: Some("cal-b".into()),
            guests: Some(Vec::new()),
            meet_request: Some("r".into()),
            ..Default::default()
        };
        assert!(!empty.names_fields());
        // Nor is "Replace with plain text…" without a description (C19).
        let flag_only = EventPatchDraft {
            replace_formatting: true,
            ..only.clone()
        };
        assert!(!flag_only.names_fields());
        for with in [
            EventPatchDraft {
                meet: Some(MeetChange::Add),
                ..only.clone()
            },
            EventPatchDraft {
                colour: Some(ColourDraft::Calendar),
                ..only.clone()
            },
            EventPatchDraft {
                guests_can_modify: Some(true),
                ..only.clone()
            },
        ] {
            assert!(with.names_fields());
        }
    }

    /// C19: "Replace with plain text…" travels as a flag beside the description; a draft from an
    /// editor that never offered it reads as not replacing.
    #[test]
    fn a_draft_reads_whether_it_replaces_formatting() {
        let replacing: EventPatchDraft =
            serde_json::from_str(r#"{"description":"Room 4.12","replace_formatting":true}"#)
                .unwrap();
        assert!(replacing.replace_formatting);
        assert_eq!(replacing.description.as_deref(), Some("Room 4.12"));
        let plain: EventPatchDraft = serde_json::from_str(r#"{"description":"Room"}"#).unwrap();
        assert!(!plain.replace_formatting);
        // The choice token binds the draft's Debug form: a token for one never fits the other.
        let keeping = EventPatchDraft {
            replace_formatting: false,
            ..replacing.clone()
        };
        assert!(
            format!("{replacing:?}") != format!("{keeping:?}"),
            "the flag is part of what a choice token binds"
        );
    }

    /// C18: a draft names a call change as the editor's row sends it, and never the request's id: the
    /// webview can't set it (R5), so a JSON that names one deserialises without it.
    #[test]
    fn a_draft_reads_a_call_change_and_never_a_request_id() {
        let add: EventPatchDraft = serde_json::from_str(r#"{"meet":"add"}"#).unwrap();
        assert_eq!(add.meet, Some(MeetChange::Add));
        let remove: EventPatchDraft = serde_json::from_str(r#"{"meet":"remove"}"#).unwrap();
        assert_eq!(remove.meet, Some(MeetChange::Remove));
        for json in [r#"{"summary":"Gym"}"#, r#"{"meet":null}"#] {
            let untouched: EventPatchDraft = serde_json::from_str(json).unwrap();
            assert!(untouched.meet.is_none(), "{json}");
        }
        assert!(serde_json::from_str::<EventPatchDraft>(r#"{"meet":"zoom"}"#).is_err());
        let named: EventPatchDraft =
            serde_json::from_str(r#"{"meet":"add","meet_request":"abc"}"#).unwrap();
        assert!(
            named.meet_request.is_none(),
            "the webview named the request"
        );
        assert_eq!(named.meet, Some(MeetChange::Add));
    }

    /// C18b: a new event asks for no call unless the editor says so, and never names the request's
    /// id: the webview can't set it (R5).
    #[test]
    fn a_new_event_reads_meet_off_by_default() {
        let time = r#""time":{"kind":"all_day","first_day":"2026-10-26","last_day":"2026-10-26"}"#;
        let plain: NewEventDraft = serde_json::from_str(&format!("{{{time}}}")).unwrap();
        assert!(!plain.meet && plain.meet_request.is_none());
        let asked: NewEventDraft =
            serde_json::from_str(&format!(r#"{{{time},"meet":true,"meet_request":"abc"}}"#))
                .unwrap();
        assert!(asked.meet, "the editor asked for a call");
        assert!(
            asked.meet_request.is_none(),
            "the webview named the request"
        );
    }

    /// C18: the call as the editor names its kind and state.
    #[test]
    fn a_call_reads_as_the_editor_names_it() {
        let call = CallForEdit {
            kind: CallKind::AddOn,
            name: "Zoom Meeting".into(),
            join: None,
            state: CallState::Pending,
        };
        assert_eq!(
            serde_json::to_string(&call).unwrap(),
            r#"{"kind":"add_on","name":"Zoom Meeting","join":null,"state":"pending"}"#
        );
        for (kind, json) in [
            (CallKind::Meet, r#""meet""#),
            (CallKind::Other, r#""other""#),
        ] {
            assert_eq!(serde_json::to_string(&kind).unwrap(), json);
        }
        for (state, json) in [
            (CallState::Ready, r#""ready""#),
            (CallState::Failed, r#""failed""#),
        ] {
            assert_eq!(serde_json::to_string(&state).unwrap(), json);
        }
    }

    /// What the editor sends for a new event: text and choices it leaves out take their defaults;
    /// a time of a kind PM doesn't know is refused, never guessed.
    #[test]
    fn new_event_draft_reads_what_the_editor_sends() {
        let full: NewEventDraft = serde_json::from_str(
            r#"{"summary":"Dentist","time":{"kind":"timed","start_date":"2026-10-26",
                "start_time":"10:00","start_zone":"Europe/London","end_date":"2026-10-26",
                "end_time":"11:00","end_zone":"Europe/London"},"location":"High St",
                "description":"Bring the form","show_as":"free","visibility":"private",
                "recurrence":{"pattern":{"kind":"weekly","days":["mon"]},"interval":1,
                "ends":{"kind":"after","count":10}}}"#,
        )
        .unwrap();
        assert!(full.summary == "Dentist" && full.location == "High St");
        assert!(full.description == "Bring the form");
        assert!(matches!(full.time, TimeDraft::Timed { .. }));
        assert_eq!(full.show_as, Some(ShowAs::Free));
        assert_eq!(full.visibility, Some(Visibility::Private));
        assert!(full
            .recurrence
            .is_some_and(|rule| rule.interval == 1 && rule.to_spec().is_ok()));

        // C12d: guests in the order added, optional only when it says so, and the permissions.
        let invited: NewEventDraft = serde_json::from_str(
            r#"{"time":{"kind":"all_day","first_day":"2026-10-26","last_day":"2026-10-26"},
                "guests":[{"email":"b@example.org","optional":true},{"email":"a@example.org"}],
                "guests_can_modify":true,"guests_can_invite_others":null}"#,
        )
        .unwrap();
        assert!(
            invited.guests
                == [
                    NewGuest {
                        email: "b@example.org".into(),
                        optional: true,
                    },
                    NewGuest {
                        email: "a@example.org".into(),
                        optional: false,
                    },
                ],
            "both guests, in order, the second required"
        );
        assert_eq!(invited.guests_can_modify, Some(true));
        assert_eq!(invited.guests_can_invite_others, None);
        assert_eq!(invited.guests_can_see_other_guests, None);
        let nameless = serde_json::from_str::<NewEventDraft>(
            r#"{"time":{"kind":"all_day","first_day":"2026-10-26","last_day":"2026-10-26"},
                "guests":[{"optional":true}]}"#,
        );
        assert!(nameless.is_err(), "a guest without an address is refused");

        let bare: NewEventDraft = serde_json::from_str(
            r#"{"time":{"kind":"all_day","first_day":"2026-10-26","last_day":"2026-10-26"}}"#,
        )
        .unwrap();
        assert!(bare.summary.is_empty() && bare.location.is_empty());
        assert!(bare.description.is_empty());
        assert_eq!((bare.show_as, bare.visibility), (None, None));
        // Left out or null, it doesn't repeat; left out, it has no guests and Google's permissions.
        assert!(bare.recurrence.is_none());
        assert!(bare.guests.is_empty());
        assert_eq!(
            (
                bare.guests_can_modify,
                bare.guests_can_invite_others,
                bare.guests_can_see_other_guests
            ),
            (None, None, None)
        );
        let null: NewEventDraft = serde_json::from_str(
            r#"{"time":{"kind":"all_day","first_day":"2026-10-26","last_day":"2026-10-26"},
                "recurrence":null}"#,
        )
        .unwrap();
        assert!(null.recurrence.is_none());

        let unknown = serde_json::from_str::<NewEventDraft>(
            r#"{"summary":"x","time":{"kind":"floating","day":"2026-10-26"}}"#,
        );
        assert!(unknown.is_err(), "an unknown kind of time is refused");
        assert!(serde_json::from_str::<NewEventDraft>(r#"{"summary":"x"}"#).is_err());
    }

    #[test]
    fn nothing_is_editable_under_a_blanket_reason() {
        let none = FieldPermissions::none(ReadOnlyReason::NotGoogle);
        assert!(!none.any());
        assert_eq!(none.reasons, vec![ReadOnlyReason::NotGoogle]);
        // The guest list alone is something that may change, and so are notifications or the
        // colour alone (C13).
        for one in [
            FieldPermissions {
                guests: true,
                ..Default::default()
            },
            FieldPermissions {
                reminders: true,
                ..Default::default()
            },
            FieldPermissions {
                colour: true,
                ..Default::default()
            },
            // How it repeats, alone (C14 critique C1-H2: its own right now, not the time's).
            FieldPermissions {
                repeat: true,
                ..Default::default()
            },
            // The video call, alone (C18).
            FieldPermissions {
                meet: true,
                ..Default::default()
            },
            // The calendar, alone (C18d).
            FieldPermissions {
                calendar: true,
                ..Default::default()
            },
        ] {
            assert!(one.any());
        }
        // A reply alone is no field of the editor's (C14): the list carries it apart.
        let reply = FieldPermissions {
            respond: true,
            ..Default::default()
        };
        assert!(!reply.any());
        assert!(!FieldPermissions::none(ReadOnlyReason::NotOrganizer).respond);
    }

    /// C14c: the editor's Repeat row reads its own right by this name (`canEdit`'s `repeat`); under
    /// any other the row would stay off on every event.
    #[test]
    fn the_repeat_right_goes_to_the_editor_by_its_name() {
        let open = FieldPermissions {
            repeat: true,
            ..Default::default()
        };
        let json = serde_json::to_value(&open).unwrap();
        assert_eq!(json["repeat"], serde_json::json!(true));
        assert_eq!(json["time"], serde_json::json!(false));
    }

    /// C20: the editor reads `one_day` beside the fields, and it is no field of its own: a birthday
    /// whose date alone may change to one day is still "the time".
    #[test]
    fn one_day_serialises_beside_the_fields() {
        let json = serde_json::to_value(FieldPermissions::default()).unwrap();
        assert_eq!(json["one_day"], serde_json::json!(false));
        let only = FieldPermissions {
            one_day: true,
            ..Default::default()
        };
        assert!(!only.any(), "one_day narrows the time, it isn't a field");
        assert_eq!(
            serde_json::to_value(&only).unwrap()["one_day"],
            serde_json::json!(true)
        );
    }

    /// C20: one whole day is all day, its first and last day the same.
    #[test]
    fn one_whole_day_is_all_day_and_one_day() {
        let all_day = |first: &str, last: &str| TimeDraft::AllDay {
            first_day: first.into(),
            last_day: last.into(),
        };
        assert!(all_day("2026-05-04", "2026-05-04").is_one_whole_day());
        assert!(!all_day("2026-05-04", "2026-05-05").is_one_whole_day());
        let timed = TimeDraft::Timed {
            start_date: "2026-05-04".into(),
            start_time: "00:00".into(),
            start_zone: "Europe/London".into(),
            end_date: "2026-05-05".into(),
            end_time: "00:00".into(),
            end_zone: "Europe/London".into(),
        };
        assert!(!timed.is_one_whole_day());
    }

    /// C14: the reply control's three words, and its two scopes; "no reply yet", Google's own words
    /// and "this and following" are refused at the boundary, never guessed.
    #[test]
    fn a_reply_reads_as_the_control_sends_it() {
        for (json, reply, google) in [
            (r#""yes""#, Rsvp::Yes, "accepted"),
            (r#""no""#, Rsvp::No, "declined"),
            (r#""maybe""#, Rsvp::Maybe, "tentative"),
        ] {
            assert_eq!(serde_json::from_str::<Rsvp>(json).unwrap(), reply);
            assert_eq!(reply.as_google(), google);
        }
        for refused in [r#""needs_action""#, r#""accepted""#, r#""Yes""#] {
            assert!(serde_json::from_str::<Rsvp>(refused).is_err(), "{refused}");
        }
        assert_eq!(
            serde_json::from_str::<RsvpScope>(r#""this""#).unwrap(),
            RsvpScope::This
        );
        assert_eq!(
            serde_json::from_str::<RsvpScope>(r#""all""#).unwrap(),
            RsvpScope::All
        );
        assert!(serde_json::from_str::<RsvpScope>(r#""following""#).is_err());
    }

    /// The list's rights sit flat beside the row's own fields, as the calendar view reads them.
    #[test]
    fn list_rights_serialise_flat() {
        assert_eq!(
            serde_json::to_string(&ListRights {
                edit_block: None,
                can_delete: true,
                can_respond: false,
                can_retime: true,
            })
            .unwrap(),
            r#"{"edit_block":null,"can_delete":true,"can_respond":false,"can_retime":true}"#
        );
        assert_eq!(
            serde_json::to_string(&ListRights {
                edit_block: Some(ReadOnlyReason::NotOrganizer),
                can_delete: false,
                can_respond: true,
                can_retime: false,
            })
            .unwrap(),
            r#"{"edit_block":"not_organizer","can_delete":false,"can_respond":true,"can_retime":false}"#
        );
        assert_eq!(
            serde_json::to_string(&ReplyForEdit {
                response: Some("needsAction".into()),
                note: String::new(),
            })
            .unwrap(),
            r#"{"response":"needsAction","note":""}"#
        );
    }

    /// C13: notifications and a colour as the editor will send them; left out, each is untouched,
    /// and a colour of a kind PM doesn't set (a label) is refused, never guessed.
    #[test]
    fn reminders_and_colour_drafts_read_what_the_editor_sends() {
        let draft: EventPatchDraft = serde_json::from_str(
            r#"{"reminders":{"use_default":false,"overrides":[{"method":"popup","minutes":10}]},
                "colour":{"kind":"colour","id":"5"}}"#,
        )
        .unwrap();
        assert!(
            draft.reminders
                == Some(RemindersDraft {
                    use_default: false,
                    overrides: vec![crate::calendar::Reminder {
                        method: "popup".into(),
                        minutes: 10,
                    }],
                }),
            "the event's own list"
        );
        assert!(draft.colour == Some(ColourDraft::Colour { id: "5".into() }));
        let calendar: ColourDraft = serde_json::from_str(r#"{"kind":"calendar"}"#).unwrap();
        assert!(calendar == ColourDraft::Calendar);
        // The defaults need no list.
        let defaults: RemindersDraft = serde_json::from_str(r#"{"use_default":true}"#).unwrap();
        assert!(defaults.use_default && defaults.overrides.is_empty());
        for refused in [
            r#"{"colour":{"kind":"label","id":"x"}}"#,
            r#"{"colour":{"kind":"tomato"}}"#,
            r#"{"reminders":{"overrides":[]}}"#,
        ] {
            assert!(
                serde_json::from_str::<EventPatchDraft>(refused).is_err(),
                "{refused}"
            );
        }
        let untouched: EventPatchDraft = serde_json::from_str(r#"{"summary":"Gym"}"#).unwrap();
        assert!(untouched.reminders.is_none() && untouched.colour.is_none());
        let new: NewEventDraft = serde_json::from_str(
            r#"{"time":{"kind":"all_day","first_day":"2026-10-26","last_day":"2026-10-26"}}"#,
        )
        .unwrap();
        assert!(new.reminders.is_none() && new.colour.is_none());
        let null: NewEventDraft = serde_json::from_str(
            r#"{"time":{"kind":"all_day","first_day":"2026-10-26","last_day":"2026-10-26"},
                "reminders":null,"colour":null}"#,
        )
        .unwrap();
        assert!(null.reminders.is_none() && null.colour.is_none());
        let coloured: NewEventDraft = serde_json::from_str(
            r#"{"time":{"kind":"all_day","first_day":"2026-10-26","last_day":"2026-10-26"},
                "reminders":{"use_default":false,"overrides":[]},"colour":{"kind":"colour","id":"11"}}"#,
        )
        .unwrap();
        assert!(coloured
            .reminders
            .is_some_and(|r| !r.use_default && r.overrides.is_empty()));
        assert!(coloured.colour == Some(ColourDraft::Colour { id: "11".into() }));
    }

    /// C12: guest changes come as changes to Google's own list, each a wanted state; a state PM
    /// doesn't know is refused, never guessed, and untouched guests are no change.
    #[test]
    fn a_draft_reads_guest_ops_as_the_editor_sends_them() {
        let draft: EventPatchDraft = serde_json::from_str(
            r#"{"guests":[{"email":"A@x.org","to":"optional"},{"email":"b@x.org","to":"absent"}],
                "guests_can_modify":true}"#,
        )
        .unwrap();
        assert!(
            draft.guest_ops()
                == [
                    GuestOp {
                        email: "A@x.org".into(),
                        to: GuestState::Optional
                    },
                    GuestOp {
                        email: "b@x.org".into(),
                        to: GuestState::Absent
                    }
                ],
            "both changes, as sent"
        );
        assert_eq!(draft.guests_can_modify, Some(true));
        assert!(draft.touches_guests());
        assert!(serde_json::from_str::<EventPatchDraft>(
            r#"{"guests":[{"email":"a@x.org","to":"maybe"}]}"#
        )
        .is_err());
        let title: EventPatchDraft = serde_json::from_str(r#"{"summary":"Gym"}"#).unwrap();
        assert!(title.guests.is_none() && !title.touches_guests());
        // An empty list reads as untouched.
        let empty: EventPatchDraft = serde_json::from_str(r#"{"guests":[]}"#).unwrap();
        assert!(empty.guest_ops().is_empty() && !empty.touches_guests());
        // Each permission alone touches the guests.
        for json in [
            r#"{"guests_can_modify":false}"#,
            r#"{"guests_can_invite_others":false}"#,
            r#"{"guests_can_see_other_guests":false}"#,
        ] {
            let one: EventPatchDraft = serde_json::from_str(json).unwrap();
            assert!(one.touches_guests(), "{json}");
        }
    }

    /// The editor names the user's own entry as Google does.
    #[test]
    fn a_guest_reads_self_as_google_names_it() {
        let me = GuestForEdit {
            email: "me@x.org".into(),
            name: None,
            response: Some("accepted".into()),
            optional: false,
            organizer: true,
            is_self: true,
            room: false,
        };
        assert_eq!(
            serde_json::to_string(&me).unwrap(),
            r#"{"email":"me@x.org","name":null,"response":"accepted","optional":false,"organizer":true,"self":true,"room":false}"#
        );
    }

    /// C12 critiques C1-M5, C2-H3: an answer stands for the numbers the question showed, by who
    /// hears, how many emails, and whether PM saw everyone; how they split doesn't matter.
    #[test]
    fn an_answer_stands_for_the_numbers_it_was_given_for() {
        let asked = GuestNotice {
            updated: 1,
            people: 1,
            emails: 1,
            complete: true,
            ..Default::default()
        };
        assert!(asked.answered_by(&asked));
        let read: GuestNotice = serde_json::from_str(
            r#"{"updated":1,"invited":0,"removed":0,"told_cancelled":0,"cancelled":false,"people":1,"emails":1,
                "complete":true}"#,
        )
        .unwrap();
        assert!(asked.answered_by(&read));
        for other in [
            GuestNotice {
                people: 2,
                ..asked.clone()
            },
            GuestNotice {
                emails: 2,
                ..asked.clone()
            },
            GuestNotice {
                complete: false,
                ..asked.clone()
            },
        ] {
            assert!(!other.answered_by(&asked));
        }
    }
}
