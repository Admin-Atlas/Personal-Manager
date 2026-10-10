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
    /// Anything else, with Google's message clipped to plain text.
    Failed {
        message: String,
    },
}

/// Saved, but with something the user should know.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WriteWarning {
    /// Google took the change, but the calendar couldn't be refreshed yet; the next sync will.
    MirrorRefreshPending,
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
                warnings: vec![WriteWarning::MirrorRefreshPending]
            })
            .unwrap(),
            r#"{"outcome":"saved","warnings":["mirror_refresh_pending"]}"#
        );
        for (outcome, json) in [
            (WriteOutcome::NoChange, r#"{"outcome":"no_change"}"#),
            (WriteOutcome::Gone, r#"{"outcome":"gone"}"#),
            (WriteOutcome::Busy, r#"{"outcome":"busy"}"#),
            (WriteOutcome::Reauth, r#"{"outcome":"reauth"}"#),
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
        let draft: TimeDraft = serde_json::from_str(
            r#"{"kind":"all_day","first_day":"2026-10-10","last_day":"2026-10-12"}"#,
        )
        .unwrap();
        assert!(matches!(draft, TimeDraft::AllDay { .. }));
    }

    #[test]
    fn nothing_is_editable_under_a_blanket_reason() {
        let none = FieldPermissions::none(ReadOnlyReason::NotGoogle);
        assert!(!none.any());
        assert_eq!(none.reasons, vec![ReadOnlyReason::NotGoogle]);
    }
}
