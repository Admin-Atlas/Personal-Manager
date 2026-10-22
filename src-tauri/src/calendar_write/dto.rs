// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! What a calendar edit is made of, and what a save returns. The webview sends drafts that name only
//! what the user touched; it never sends a provider id, an etag or a raw Google field (those stay in
//! the backend's edit session). Outcomes come back as data (`Ok(WriteOutcome)`), so the editor can
//! keep the draft and explain, rather than read an error string.

use serde::{Deserialize, Serialize};

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

/// The fields the user touched in the editor. `None` means untouched and is never sent; `Some` of
/// the value already on the event is dropped too, so nothing Google holds is rewritten for nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventPatchDraft {
    pub summary: Option<String>,
    /// `Some("")` clears it.
    pub location: Option<String>,
    /// Plain text. `Some("")` clears it.
    pub description: Option<String>,
    pub time: Option<TimeDraft>,
    pub show_as: Option<ShowAs>,
    pub visibility: Option<Visibility>,
}

/// A new event as the editor holds it (A22: defined with its first caller, the create command). No
/// repeat (C11), guests (C12), reminders or colour (C13): Google gives the calendar's default
/// reminders when none are sent. Empty text is left out of the insert, never sent as "".
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct NewEventDraft {
    #[serde(default)]
    pub summary: String,
    pub time: TimeDraft,
    #[serde(default)]
    pub location: String,
    /// Plain text.
    #[serde(default)]
    pub description: String,
    /// `None` is Google's default (busy).
    #[serde(default)]
    pub show_as: Option<ShowAs>,
    /// `None` or `Default` is the calendar's default, and isn't sent.
    #[serde(default)]
    pub visibility: Option<Visibility>,
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

/// Which occurrences of a repeating event a save or a delete is for: the dialog's three rows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeriesScope {
    This,
    Following,
    All,
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
    /// Someone else organises it: an invitation.
    NotOrganizer,
    /// A one-off in PM that has since become a repeating event in Google: open one of its occurrences.
    BecameRepeating,
    /// It has guests, who Google would email (until Send / Don't send lands).
    HasGuests,
    /// A birthday, a Gmail event, focus time, out of office or a working location.
    SpecialType,
    /// Google has locked its title, time, place and description.
    Locked,
    /// The description is formatted (HTML); editing it as plain text would strip that.
    HtmlDescription,
}

/// What may change on one event, field by field, and why the rest can't. The backend is the
/// authority: the editor greys out what this says, and every save checks it again on a fresh copy.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct FieldPermissions {
    pub summary: bool,
    pub time: bool,
    pub location: bool,
    pub description: bool,
    pub show_as: bool,
    pub visibility: bool,
    pub delete: bool,
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

    /// Whether anything at all may change.
    pub fn any(&self) -> bool {
        self.summary
            || self.time
            || self.location
            || self.description
            || self.show_as
            || self.visibility
            || self.delete
    }
}

/// What a save came to, as data. `Saved` and `NoChange` are the successes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum WriteOutcome {
    Saved {
        warnings: Vec<WriteWarning>,
        /// Occurrences changed on their own that PM couldn't cancel (`YYYY-MM-DD`, in the series'
        /// zone): they may still show. Empty, and left out, for every other save.
        #[serde(skip_serializing_if = "Vec::is_empty")]
        left_behind: Vec<String>,
        /// A "this and following" change that landed, with milestones linked to the earlier part
        /// dated on or after the day it starts, except one on an occurrence it left there: the
        /// webview asks about each (DECISIONS 17-10 #2).
        /// `None`, and left out, for every other save, and for a split with nothing to offer. Boxed,
        /// so every outcome (and every `Result` that carries one as its error) stays small.
        #[serde(skip_serializing_if = "Option::is_none")]
        milestone_moves: Option<Box<MilestoneMoves>>,
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
    /// The plan for the occurrences chosen isn't the one the dialog showed any more, or none was
    /// named: choose again. Nothing was sent.
    NeedsChoice,
    /// Google answered as if it had saved the change, but kept the event as it was. `split`: the new
    /// part a "this and following" change made was removed too.
    IgnoredByGoogle {
        split: bool,
    },
    /// A "this and following" change PM couldn't finish: Google may hold part of it. `token` names
    /// it for settling; `offers` is what Google's state allows now; `from` is the day the change
    /// starts (`YYYY-MM-DD`, in the series' zone). Never an id or an etag (R5). `note`: a settle
    /// that didn't end the change says why here, while PM still holds it to settle (so its buttons
    /// stay); left out for every other answer.
    PartialSplit {
        token: String,
        offers: Vec<SplitAction>,
        from: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        note: Option<String>,
    },
    /// The new part of a change PM couldn't finish was removed: the repeating event is as it was.
    SplitRemoved,
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
    /// As Google holds it. When `description_html` is set it's formatted, and shown read-only.
    pub description: String,
    pub description_html: bool,
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
        /// change someone made to the series after the editor opened (G9).
        series: Option<SeriesInfo>,
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
}

/// What the choices for a change to a repeating occurrence are asked for: an open editor's draft, or
/// a delete of a mirror row (`seen` is what the user saw of it).
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ChoiceRequest {
    Edit {
        session: String,
        draft: EventPatchDraft,
    },
    Delete {
        event_id: String,
        seen: SeenSummary,
    },
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
    /// changed since gets asked about again.
    Offered {
        warnings: Vec<SeriesWarningDto>,
        token: String,
    },
    /// "This and following" from the series' first occurrence: every occurrence, as "All events".
    AsAll {
        warnings: Vec<SeriesWarningDto>,
        token: String,
    },
    /// Not for this change, and why, in words.
    Refused { message: String },
    /// Changed in Google since the editor opened, on these fields.
    Conflict { fields: Vec<String> },
    /// A change PM couldn't finish holds this series (C9 design review M1): settle it first, as
    /// [`WriteOutcome::PartialSplit`] says.
    Pending {
        token: String,
        offers: Vec<SplitAction>,
        from: String,
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
                reason: ReadOnlyReason::HtmlDescription
            })
            .unwrap(),
            r#"{"outcome":"read_only","reason":"html_description"}"#
        );
        assert_eq!(
            serde_json::to_string(&WriteOutcome::ReadOnly {
                reason: ReadOnlyReason::BecameRepeating
            })
            .unwrap(),
            r#"{"outcome":"read_only","reason":"became_repeating"}"#
        );
        assert_eq!(
            serde_json::to_string(&WriteOutcome::Saved {
                warnings: vec![WriteWarning::MirrorRefreshPending],
                left_behind: Vec::new(),
                milestone_moves: None,
            })
            .unwrap(),
            r#"{"outcome":"saved","warnings":["mirror_refresh_pending"]}"#
        );
        assert_eq!(
            serde_json::to_string(&WriteOutcome::Saved {
                warnings: vec![WriteWarning::LeftOnOldSeries],
                left_behind: vec!["2026-11-02".into()],
                milestone_moves: None,
            })
            .unwrap(),
            r#"{"outcome":"saved","warnings":["left_on_old_series"],"left_behind":["2026-11-02"]}"#
        );
        for (outcome, json) in [
            (WriteOutcome::NeedsChoice, r#"{"outcome":"needs_choice"}"#),
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
                },
                r#"{"outcome":"partial_split","token":"t","offers":["remove","check"],"from":"2026-11-02"}"#,
            ),
            (
                WriteOutcome::PartialSplit {
                    token: "t".into(),
                    offers: vec![SplitAction::Check],
                    from: "2026-11-02".into(),
                    note: Some("Try again in a moment.".into()),
                },
                r#"{"outcome":"partial_split","token":"t","offers":["check"],"from":"2026-11-02","note":"Try again in a moment."}"#,
            ),
            (WriteOutcome::SplitRemoved, r#"{"outcome":"split_removed"}"#),
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
            })
            .unwrap(),
            r#"{"outcome":"saved","warnings":["milestone_not_offered"],"milestone_moves":{"new_series_ical":null,"earlier_series_ical":"abc@google.com","new_title":"Gym","earlier_title":"Gym","first_day":null,"earlier_last_day":null,"today":"2026-10-13","milestones":[{"id":7,"label":"Launch","project":"Atlas","date":"2026-10-19"}]}}"#
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
            },
            following: ScopeRow::AsAll {
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
            })
            .unwrap(),
            r#"{"row":"pending","token":"t","offers":["finish"],"from":"2026-11-02"}"#
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
                repeats: "Weekly on Monday".into()
            })
            .unwrap(),
            r#"{"first":false,"repeats":"Weekly on Monday"}"#
        );
    }

    /// What the editor sends for a new event: text and choices it leaves out take their defaults;
    /// a time of a kind PM doesn't know is refused, never guessed.
    #[test]
    fn new_event_draft_reads_what_the_editor_sends() {
        let full: NewEventDraft = serde_json::from_str(
            r#"{"summary":"Dentist","time":{"kind":"timed","start_date":"2026-10-26",
                "start_time":"10:00","start_zone":"Europe/London","end_date":"2026-10-26",
                "end_time":"11:00","end_zone":"Europe/London"},"location":"High St",
                "description":"Bring the form","show_as":"free","visibility":"private"}"#,
        )
        .unwrap();
        assert!(full.summary == "Dentist" && full.location == "High St");
        assert!(full.description == "Bring the form");
        assert!(matches!(full.time, TimeDraft::Timed { .. }));
        assert_eq!(full.show_as, Some(ShowAs::Free));
        assert_eq!(full.visibility, Some(Visibility::Private));

        let bare: NewEventDraft = serde_json::from_str(
            r#"{"time":{"kind":"all_day","first_day":"2026-10-26","last_day":"2026-10-26"}}"#,
        )
        .unwrap();
        assert!(bare.summary.is_empty() && bare.location.is_empty());
        assert!(bare.description.is_empty());
        assert_eq!((bare.show_as, bare.visibility), (None, None));

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
    }
}
