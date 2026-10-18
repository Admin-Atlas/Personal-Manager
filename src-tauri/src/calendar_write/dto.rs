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
    /// A repeating event (until "this / following / all" lands).
    Recurring,
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
    /// starts (`YYYY-MM-DD`, in the series' zone). Never an id or an etag (R5).
    PartialSplit {
        token: String,
        offers: Vec<SplitAction>,
        from: String,
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
            serde_json::to_string(&WriteOutcome::Saved {
                warnings: vec![WriteWarning::MirrorRefreshPending],
                left_behind: Vec::new(),
            })
            .unwrap(),
            r#"{"outcome":"saved","warnings":["mirror_refresh_pending"]}"#
        );
        assert_eq!(
            serde_json::to_string(&WriteOutcome::Saved {
                warnings: vec![WriteWarning::LeftOnOldSeries],
                left_behind: vec!["2026-11-02".into()],
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
                },
                r#"{"outcome":"partial_split","token":"t","offers":["remove","check"],"from":"2026-11-02"}"#,
            ),
            (WriteOutcome::SplitRemoved, r#"{"outcome":"split_removed"}"#),
            (WriteOutcome::NoChange, r#"{"outcome":"no_change"}"#),
            (WriteOutcome::Gone, r#"{"outcome":"gone"}"#),
            (WriteOutcome::Busy, r#"{"outcome":"busy"}"#),
            (WriteOutcome::Reauth, r#"{"outcome":"reauth"}"#),
            (WriteOutcome::Unconfirmed, r#"{"outcome":"unconfirmed"}"#),
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

    #[test]
    fn nothing_is_editable_under_a_blanket_reason() {
        let none = FieldPermissions::none(ReadOnlyReason::NotGoogle);
        assert!(!none.any());
        assert_eq!(none.reasons, vec![ReadOnlyReason::NotGoogle]);
    }
}
