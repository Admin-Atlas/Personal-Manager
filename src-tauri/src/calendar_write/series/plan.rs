// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! The writes a change to a repeating event takes, in order, before any is sent (T8, T9).
//!
//! - **This event**: one PATCH (or a cancel) of the occurrence.
//! - **All events**: one PATCH (or DELETE) of the series' master. A new time moves the rule's repeat
//!   day, the start, the end and every EXDATE with it ([`retime`]).
//! - **This and following**: Google documents no split, so it follows the prior art. A new series
//!   (the "tail") is inserted first, then the original is cut to end just before the occurrence; then
//!   each occurrence after it that was changed on its own is carried onto the tail and cancelled on the
//!   original. Insert first, so a failure leaves a visible duplicate, never missing occurrences.
//!
//! Each [`Step`] says how to read its answer, because the executor (C9) must never undo on a guess
//! (DS4): a step is done, not done, or unknown. A reply that may have been applied is checked with a
//! GET ([`SeriesPlan::verdict`]) before any 412 rule ([`SeriesPlan::resend`]); only a step proved not
//! done ([`Applied::No`]) undoes the steps before it, in reverse, each sent with that step's own
//! `sendUpdates` (A3, R7b); an unknown one stops the run, and a tail is never deleted on it.
//!
//! The rules C9 runs a plan by, beyond that (C8b reviews):
//!
//! - **Proof of not done is a definite answer**: a 2xx holding the step's `old`; a 412 whose
//!   [`SeriesPlan::resend`] is a conflict; a 404/410 whose verdict with `None` is not done (a step
//!   that isn't itself a cancel or a delete: for those, gone is done); or an answer classify calls
//!   final before anything is applied (`BadRequest`, `ReadOnly`, `NotOrganizer`,
//!   `InsufficientScope`, `QuotaExceeded`, `Reauth`, and `RateLimited` after its one retry). A lost
//!   answer or a 5xx isn't: a GET that still shows `old` may be followed by the request still in
//!   flight, so the same request goes once more on its original If-Match. A late first copy makes that
//!   a 412, and the GET shows it landed.
//! - **Removing a new series is an undo like any other.** After an unknown cut (C9's partial split),
//!   "Remove the new series" GETs the original. Showing `old` proves the cut not done only at an etag
//!   other than the one the cut was sent on (a copy still in flight would fail its If-Match); at the
//!   same etag, C9 first moves it with a fence write on the cut's If-Match (a PATCH of a private
//!   property, notify none). A 2xx holding the old rule proves the cut not done; a 412 says only that
//!   the etag moved, so C9 GETs again and takes the cut's verdict: the cut's rule is done, `old` at an
//!   etag other than the cut's is proved not done, anything else unknown. Proved not done, the tail
//!   may be deleted; done offers "Finish", the rest of this same plan from the step after the cut;
//!   unknown offers neither. Never re-send the cut to settle it: that turns Remove into Finish against
//!   the user's choice.
//! - **One plan, one tail id.** Every new plan mints a new tail id; an id is reused only to send the
//!   same plan again. A series with an unresolved partial split takes no new plan for "this and
//!   following" or "all events" until Remove or Finish settles it. The insert is proved by the plan's
//!   own note on the new series ([`Shows::Tail`]), so a tail from another plan never reads as this one.
//! - **An undo has its own rules** ([`Undo`]): a delete that meets 404/410 is done; on a 412 it's
//!   sent once more only while the event is still PM's own new series.
//! - **A 2xx DELETE has no body**: C9 counts it done, or passes `None` to the verdict.
//! - **Carries run after the cut**, so a carry hiccup (a list that lags just after the insert) leaves
//!   a visible duplicate rather than stopping a split with every later occurrence doubled. C9 looks
//!   a carry's occurrence up again after 1 s and 3 s. If C9a's probes (L-R2, L-R5) show Google drops
//!   or hides an original's exceptions past a new UNTIL, the order changes: carries become Commit
//!   steps before the cut.
//! - **The mirror** takes [`SeriesPlan::expect_after`] and [`SeriesPlan::removes_for`] once the run is
//!   over: a split's removals wait for its new series, and an occurrence the run couldn't cancel stays.
//!   Removed rows carry a version floor ([`super::diff::GoneRow`], [`super::diff::floor_of`]).

use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use chrono_tz::Tz;
use serde_json::{json, Map, Value};

use crate::calendar_recur::expand::{is_occurrence, last_before};
use crate::calendar_recur::lines::{self, recurrence_verdict, same_recurrence, Applied, Kind};
use crate::calendar_recur::reanchor::DayMap;
use crate::calendar_recur::remap::{exdate_lines, retime, RetimeError, Retimed};
use crate::calendar_recur::split::{
    remaining_count, tail_recurrence, truncate_recurrence, Truncation,
};
use crate::calendar_recur::{rfc_instant, Point, RecurError, SeriesStart};

use super::super::dto::{EventPatchDraft, Notify, ReadOnlyReason, SeenSummary};
use super::super::patch::{
    build_patch, delete_still_matches, drop_other_kind, is_gone, update_check, when_eq,
};
use super::super::patch::{DeleteCheck, UpdateCheck};
use super::super::plan::{
    delete_event, insert_event, is_plain_segment, patch_event, RequestPlan, VersionFlags,
};
use super::super::time;
use super::diff::{Expect, Removes, SeriesDiff};
use super::scope::{is_first, zones_changed, Scope};
use super::{
    attendee_set, classify_instance, google_before, original_of, point_of, Exceptions, FreshEvent,
    InstanceKind, Length, Listing, Master, SeriesRefusal,
};

/// Everything a series plan is built from. C9 makes the GETs; nothing here fetches.
#[derive(Debug, Clone, Copy)]
pub struct SeriesInput<'a> {
    /// Google's id for the calendar.
    pub calendar: &'a str,
    /// What the user opened (R2): a change made in Google since is a conflict.
    pub opened: Opened<'a>,
    /// What Google holds now, fetched for this save.
    pub fresh: Fresh<'a>,
    /// Every stored exception of the series, cancelled ones included.
    pub exceptions: &'a Exceptions,
    /// The series' instances, for rebasing a COUNT (only a series with one needs them).
    pub instances: Option<&'a Listing>,
    /// The mirror holds another part of the same series, split earlier (Google's own `<id>_R…`).
    pub sibling: bool,
    /// Whether a series with guests may change: false until C12 brings invitations, whatever the
    /// occurrence shows. One whose guests were removed on its own passes the gate, and a split would
    /// invite the series' guests again (C8b review G5).
    pub guests_allowed: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct Opened<'a> {
    pub instance: &'a FreshEvent,
    /// The series as the editor opened it, needed for "all events" and "this and following" (G9).
    pub master: Option<&'a FreshEvent>,
}

#[derive(Debug, Clone, Copy)]
pub struct Fresh<'a> {
    pub instance: &'a FreshEvent,
    pub master: &'a FreshEvent,
}

/// Why there's no plan: nothing to do, or something the user must see first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotPlanned {
    ReadOnly(ReadOnlyReason),
    /// Changed in Google since it was opened, on these fields. Nothing is overwritten.
    Conflict(Vec<String>),
    /// The occurrence or the series is gone from Google.
    Gone,
    /// Google already holds exactly this.
    NoChange,
    Refused(SeriesRefusal),
}

impl From<SeriesRefusal> for NotPlanned {
    fn from(e: SeriesRefusal) -> Self {
        NotPlanned::Refused(e)
    }
}

impl From<RecurError> for NotPlanned {
    fn from(e: RecurError) -> Self {
        NotPlanned::Refused(e.into())
    }
}

impl From<RetimeError> for NotPlanned {
    fn from(e: RetimeError) -> Self {
        NotPlanned::Refused(e.into())
    }
}

impl From<time::TimeError> for NotPlanned {
    fn from(e: time::TimeError) -> Self {
        NotPlanned::Refused(e.into())
    }
}

/// The writes, in order.
#[derive(Debug, Clone, PartialEq)]
pub struct SeriesPlan {
    /// What runs: "this and following" at the series' first occurrence runs as "all events".
    pub scope: Scope,
    pub steps: Vec<Step>,
    pub warnings: Vec<SeriesWarning>,
    /// What a re-fetch has to show before it can stand for the mirror ([`super::diff::diff_series`]).
    pub expect: Vec<Expect>,
    /// The mirror rows the plan removes (A14), as planned: the run's own are
    /// [`SeriesPlan::removes_for`].
    pub removes: Removes,
    /// The series' start, for reading recurrence lines by what they mean.
    start: SeriesStart,
}

/// One write.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub op: Op,
    /// Who Google emails (A3: every step its own, its undo the same).
    pub notify: Notify,
    pub stage: Stage,
    /// Runs only if that step is done: an exception is cancelled on the original only once it's on
    /// the new series (DS3).
    pub requires: Option<usize>,
    /// What a 412 means.
    pub conflict: Recheck,
    /// What the event shows when the step has (or hasn't) landed.
    pub shows: Shows,
    /// A 404 or 410 means done: true for a cancel or a delete, which Google calls already deleted.
    pub gone_is_done: bool,
    pub undo: Option<Undo>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    /// Not done (proved): undo the earlier Commit steps, in reverse.
    Commit,
    /// Not done: a warning, and the steps that require it are skipped.
    Cleanup,
}

/// The request a step sends.
#[derive(Debug, Clone, PartialEq)]
pub enum Op {
    /// A new series under PM's own id, minted for this plan and kept for every retry of it:
    /// re-sending the same id can't make a second series, and a 409 means it's there (DS5). A new
    /// plan gets a new id.
    Insert {
        calendar: String,
        id: String,
        body: Map<String, Value>,
    },
    Patch {
        calendar: String,
        event: String,
        etag: String,
        body: Map<String, Value>,
    },
    /// Cancel one occurrence: Google's documented route is the instance's `status` (A8 reads it as a
    /// delete). `original` is where it sits in its series.
    Cancel {
        calendar: String,
        event: String,
        etag: String,
        original: Point,
    },
    Delete {
        calendar: String,
        event: String,
        etag: String,
    },
    /// PATCH `body` onto the occurrence of `series` whose `originalStartTime` is `original`. Its id
    /// and etag come from a listing of that series (ids are opaque), so C9 looks it up first.
    Carry {
        calendar: String,
        series: String,
        original: Point,
        body: Map<String, Value>,
    },
}

impl Op {
    /// The request, sent with `notify`. `None` for a carry, which needs its occurrence found first
    /// ([`Op::carry_request`]).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "first caller lands in C9, editing repeating events"
        )
    )]
    pub fn request(&self, notify: Notify) -> Option<RequestPlan> {
        Some(match self {
            Op::Insert { calendar, id, body } => insert_event(
                calendar,
                id,
                Value::Object(body.clone()),
                notify,
                // conferenceDataVersion=1 with no conference: Google adds no Meet of its own to the
                // new series (AS16).
                VersionFlags {
                    conference: true,
                    labels: false,
                },
            ),
            Op::Patch {
                calendar,
                event,
                etag,
                body,
            } => patch_event(
                calendar,
                event,
                etag,
                Value::Object(body.clone()),
                notify,
                VersionFlags::default(),
            ),
            Op::Cancel {
                calendar,
                event,
                etag,
                ..
            } => patch_event(
                calendar,
                event,
                etag,
                json!({ "status": "cancelled" }),
                notify,
                VersionFlags::default(),
            ),
            Op::Delete {
                calendar,
                event,
                etag,
            } => delete_event(calendar, event, etag, notify),
            Op::Carry { .. } => return None,
        })
    }

    /// A carry's request, once its occurrence is found as `event` at `etag`.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "first caller lands in C9, editing repeating events"
        )
    )]
    pub fn carry_request(&self, event: &str, etag: &str, notify: Notify) -> Option<RequestPlan> {
        match self {
            Op::Carry { calendar, body, .. } if is_plain_segment(event) => Some(patch_event(
                calendar,
                event,
                etag,
                Value::Object(body.clone()),
                notify,
                VersionFlags::default(),
            )),
            _ => None,
        }
    }

    /// Whether the body holds only keys this kind of write may send; anything else is a bug, and C9
    /// refuses to send it (G5).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "first caller lands in C9, editing repeating events"
        )
    )]
    pub fn fits(&self) -> bool {
        self.body()
            .is_none_or(|b| b.keys().all(|k| self.allowed().contains(&k.as_str())))
    }

    fn allowed(&self) -> &'static [&'static str] {
        match self {
            Op::Insert { .. } => TAIL_FIELDS,
            Op::Patch { .. } => &[
                "summary",
                "location",
                "description",
                "start",
                "end",
                "transparency",
                "visibility",
                "recurrence",
            ],
            Op::Carry { .. } => &[
                "summary",
                "description",
                "location",
                "colorId",
                "transparency",
                "reminders",
                "start",
                "end",
            ],
            Op::Cancel { .. } | Op::Delete { .. } => &[],
        }
    }

    fn body(&self) -> Option<&Map<String, Value>> {
        match self {
            Op::Insert { body, .. } | Op::Patch { body, .. } | Op::Carry { body, .. } => Some(body),
            Op::Cancel { .. } | Op::Delete { .. } => None,
        }
    }
}

/// What a 412 means for a step: whether to send it once more on Google's newer version.
#[derive(Debug, Clone, PartialEq)]
pub enum Recheck {
    /// Only if these fields are still as the plan saw them.
    Unchanged(Map<String, Value>),
    /// A cancel of the occurrence the user picked: what they saw must still be what's there (A8).
    SeenStillMatches(SeenSummary),
    /// A delete of the whole series: its title, start and rule as the plan saw them, as well as what
    /// the user saw of the occurrence, which was checked before planning (A8, AS11).
    Series {
        summary: Value,
        start: Value,
        recurrence: Vec<String>,
    },
    /// Never: an insert has no etag, and a carry's occurrence was looked up just before it was sent.
    Never,
}

/// The answer to a 412.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resend {
    /// Send again on the fresh etag.
    Proceed,
    Gone,
    Conflict(Vec<String>),
}

/// What the event shows when a step has landed, and when it provably hasn't.
#[derive(Debug, Clone, PartialEq)]
pub enum Shows {
    /// `sent` when it landed; `old` (all of it) when it didn't. Anything else is unknown.
    Fields {
        sent: Map<String, Value>,
        old: Map<String, Value>,
    },
    /// Gone (404/410) or cancelled.
    Gone,
    /// There at all, and this plan's new series: split off `of` (its private `pmSplitOf` note) by the
    /// plan `plan` (`pmSplitPlan`, a digest of what the plan inserts). The id is PM's, so a 409 and a
    /// GET that finds this mean it landed; a tail another plan left under a reused id reads as
    /// unknown, never as this one (C8b fix review). Its absence once proves nothing (DS5).
    Tail { of: String, plan: String },
}

/// The compensation for a step, sent with the step's own `notify`, on the etag of the step's reply.
#[derive(Debug, Clone, PartialEq)]
pub enum Undo {
    /// Delete what the step inserted: the new series split off `split_of` by the plan `plan`.
    DeleteInserted {
        calendar: String,
        event: String,
        split_of: String,
        plan: String,
    },
    /// Put back the master's start, end and rule (DS8), and only when [`SeriesPlan::half_applied`]
    /// says Google took one of the new start and the new rule without the other, which leaves a
    /// series starting off its own rule. Never on a plain unknown, which may have landed whole.
    RestoreMaster {
        calendar: String,
        event: String,
        body: Map<String, Value>,
    },
}

impl Undo {
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "first caller lands in C9, editing repeating events"
        )
    )]
    pub fn request(&self, etag: &str, notify: Notify) -> RequestPlan {
        match self {
            Undo::DeleteInserted {
                calendar, event, ..
            } => delete_event(calendar, event, etag, notify),
            Undo::RestoreMaster {
                calendar,
                event,
                body,
            } => patch_event(
                calendar,
                event,
                etag,
                Value::Object(body.clone()),
                notify,
                VersionFlags::default(),
            ),
        }
    }

    /// Whether a 404 or 410 means the undo is done: a delete of something already deleted.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "first caller lands in C9, editing repeating events"
        )
    )]
    pub fn gone_is_done(&self) -> bool {
        matches!(self, Undo::DeleteInserted { .. })
    }

    /// A 412, `fresh` being Google's copy now: whether to send the undo once more on its etag. A
    /// guest's reply (a room accepts within seconds) moves a new series' etag, and it's still PM's to
    /// delete while it says it was split off the original; a restore meets a change and stops.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "first caller lands in C9, editing repeating events"
        )
    )]
    pub fn resend_on(&self, fresh: &Value) -> bool {
        match self {
            Undo::DeleteInserted { split_of, plan, .. } => {
                !is_gone(fresh) && is_tail_of(fresh, split_of, plan)
            }
            Undo::RestoreMaster { .. } => false,
        }
    }
}

/// Whether `event` is the new series the plan `plan` split off the series `of`.
fn is_tail_of(event: &Value, of: &str, plan: &str) -> bool {
    let note = |key: &str| {
        event
            .pointer(&format!("/extendedProperties/private/{key}"))
            .and_then(Value::as_str)
    };
    note("pmSplitOf") == Some(of) && note("pmSplitPlan") == Some(plan)
}

/// A digest of what a split inserts, minus its private notes: the same plan sent again gives the same
/// one, another plan another.
fn plan_digest(body: &Map<String, Value>) -> String {
    use sha2::{Digest, Sha256};
    let content: Map<String, Value> = body
        .iter()
        .filter(|(k, _)| k.as_str() != "extendedProperties")
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    let digest = Sha256::digest(Value::Object(content).to_string().as_bytes());
    hex::encode(&digest[..8])
}

/// What the user should know before choosing (C9 shows these on the row; DS12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SeriesWarning {
    /// Another part of this series was split off earlier; Google may treat it as one with this.
    SplitSibling,
    /// This many occurrences were changed on their own; Google may keep or reset their own changes
    /// (L-R2/L-R3 record which).
    ExceptionsMayReset(u32),
    /// The series' first occurrence moves.
    StartMoved { from: NaiveDate, to: NaiveDate },
    /// The days the series falls on move, as the occurrence at `from` moves to `to`: measured from
    /// where the occurrence first was, so retiming one that was already moved on its own moves the
    /// series' days too (AS2a). Raised for any move to another day, whatever the rule, a move of
    /// whole weeks included. C9 words it from the rule and both dates: the new weekday, day of the
    /// month or date where the rule names one ("every Sunday → every Monday?"), and how far every
    /// occurrence moves wherever that is a week or more (a one-day weekly rule may move a week and a
    /// day) or the rule names no day (daily). A split raises no `StartMoved`, so this is all it says.
    RepeatDayChanges { from: NaiveDate, to: NaiveDate },
    /// Every occurrence gets this occurrence's length.
    LengthChanges,
    /// The new series doesn't take the original's attachments, video call or label.
    NotCarried {
        attachments: bool,
        conference: bool,
        label: bool,
    },
    /// The new series is a new event in Google (a new iCal UID): links to the old one (a milestone,
    /// a reminder) stay with the part before the split.
    NewIdentity,
}

/// The fields a split's new series takes from the original, besides its times and rule.
const TAIL_COPY: &[&str] = &[
    "summary",
    "description",
    "location",
    "colorId",
    "transparency",
    "visibility",
    "reminders",
    "guestsCanInviteOthers",
    "guestsCanModify",
    "guestsCanSeeOtherGuests",
    "anyoneCanAddSelf",
    "privateCopy",
    "source",
];

/// Everything a split's new series takes from the original, its times and rule included: what may
/// not have changed in Google before the split is planned, nor before its cut is sent again (R2,
/// AS10). One list, so the two checks and the body can't drift (C8b review G1).
const TAIL_WATCH: &[&str] = &[
    "recurrence",
    "start",
    "end",
    "summary",
    "description",
    "location",
    "colorId",
    "transparency",
    "visibility",
    "reminders",
    "guestsCanInviteOthers",
    "guestsCanModify",
    "guestsCanSeeOtherGuests",
    "anyoneCanAddSelf",
    "privateCopy",
    "source",
    "attendees",
    "extendedProperties",
];

/// What a cleanup cancel would throw away: re-sent on a 412 only while all of it is as the plan saw
/// it, so a change made to that occurrence meanwhile is never cancelled with it (DS10).
const CANCEL_WATCH: &[&str] = &[
    "status",
    "summary",
    "description",
    "location",
    "colorId",
    "transparency",
    "visibility",
    "reminders",
    "start",
    "end",
    "attendees",
    "attachments",
    "conferenceData",
    "extendedProperties",
    "eventLabelId",
    "guestsCanInviteOthers",
    "guestsCanModify",
    "guestsCanSeeOtherGuests",
];

const TAIL_FIELDS: &[&str] = &[
    "summary",
    "description",
    "location",
    "colorId",
    "transparency",
    "visibility",
    "reminders",
    "guestsCanInviteOthers",
    "guestsCanModify",
    "guestsCanSeeOtherGuests",
    "anyoneCanAddSelf",
    "privateCopy",
    "source",
    "attendees",
    "extendedProperties",
    "start",
    "end",
    "recurrence",
];

impl SeriesPlan {
    /// A 412 on step `step`, `fresh` being Google's copy now: send again, or not. Asked only after
    /// [`SeriesPlan::verdict`] found the step not landed.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "first caller lands in C9, editing repeating events"
        )
    )]
    pub fn resend(&self, step: usize, fresh: &Value) -> Resend {
        let Some(step) = self.steps.get(step) else {
            return Resend::Conflict(Vec::new());
        };
        if is_gone(fresh) {
            return Resend::Gone;
        }
        let changed: Vec<String> = match &step.conflict {
            Recheck::Never => return Resend::Conflict(Vec::new()),
            Recheck::SeenStillMatches(seen) => {
                return match delete_still_matches(seen, fresh) {
                    DeleteCheck::Matches => Resend::Proceed,
                    DeleteCheck::Gone => Resend::Gone,
                    DeleteCheck::Changed(fields) => Resend::Conflict(fields),
                }
            }
            Recheck::Unchanged(base) => base
                .iter()
                .filter(|(k, v)| !self.same(k, Some(v), fresh.get(k.as_str())))
                .map(|(k, _)| k.clone())
                .collect(),
            Recheck::Series {
                summary,
                start,
                recurrence,
            } => {
                let recurrence = Value::from(recurrence.clone());
                [
                    ("summary", summary),
                    ("start", start),
                    ("recurrence", &recurrence),
                ]
                .into_iter()
                .filter(|(k, v)| !self.same(k, Some(v), fresh.get(*k)))
                .map(|(k, _)| k.to_string())
                .collect()
            }
        };
        if changed.is_empty() {
            Resend::Proceed
        } else {
            Resend::Conflict(changed)
        }
    }

    /// Whether step `step` landed, read from Google's copy (`None` for a 404 or 410): its reply, or a
    /// GET after an answer that may have been applied.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "first caller lands in C9, editing repeating events"
        )
    )]
    pub fn verdict(&self, step: usize, fresh: Option<&Value>) -> Applied {
        let Some(step) = self.steps.get(step) else {
            return Applied::Unknown;
        };
        match (&step.shows, fresh) {
            (Shows::Gone, None) => Applied::Yes,
            (Shows::Gone, Some(v)) if is_gone(v) => Applied::Yes,
            (Shows::Gone, Some(_)) => Applied::No,
            (Shows::Tail { of, plan }, Some(v)) if !is_gone(v) && is_tail_of(v, of, plan) => {
                Applied::Yes
            }
            (Shows::Tail { .. }, _) => Applied::Unknown,
            (Shows::Fields { .. }, None) if step.gone_is_done => Applied::Yes,
            // The event itself is gone: whatever was sent can't be there.
            (Shows::Fields { .. }, None) => Applied::No,
            (Shows::Fields { sent, old }, Some(v)) => {
                let holds = |fields: &Map<String, Value>| {
                    fields
                        .iter()
                        .all(|(k, want)| self.same(k, Some(want), v.get(k.as_str())))
                };
                if holds(sent) {
                    Applied::Yes
                } else if !old.is_empty() && holds(old) {
                    Applied::No
                } else {
                    Applied::Unknown
                }
            }
        }
    }

    /// Whether step `step` half landed (DS8): `fresh` holds the new start but the old rule, or the new
    /// rule but the old start, which leaves a series starting off its own rule. The one case its
    /// [`Undo::RestoreMaster`] is for, sent on `fresh`'s etag with the step's own `notify`.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "first caller lands in C9, editing repeating events"
        )
    )]
    pub fn half_applied(&self, step: usize, fresh: &Value) -> bool {
        let Some(Step {
            shows: Shows::Fields { sent, old },
            undo: Some(Undo::RestoreMaster { .. }),
            ..
        }) = self.steps.get(step)
        else {
            return false;
        };
        let (Some(new_start), Some(sent_rule)) = (sent.get("start"), sent.get("recurrence")) else {
            return false;
        };
        let start_landed = when_eq(Some(new_start), fresh.get("start"));
        let start_kept = when_eq(old.get("start"), fresh.get("start"));
        let verdict = recurrence_verdict(
            &lines_of(Some(sent_rule)),
            &lines_of(old.get("recurrence")),
            &lines_of(fresh.get("recurrence")),
            &self.start,
        );
        (start_landed && verdict == Applied::No) || (start_kept && verdict == Applied::Yes)
    }

    /// What a re-fetch has to show once the run is over, the cleanup cancels in `skipped` (step
    /// indexes) not done: their occurrences are still in Google, so a re-fetch listing them still
    /// shows the write (C8b review R2).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "first caller lands in C9, editing repeating events"
        )
    )]
    pub fn expect_after(&self, skipped: &[usize]) -> Vec<Expect> {
        let left = self.left_behind(skipped);
        self.expect
            .iter()
            .cloned()
            .map(|e| match e {
                Expect::EndsBefore { series, at, .. } => Expect::EndsBefore {
                    series,
                    at,
                    left: left.clone(),
                },
                other => other,
            })
            .collect()
    }

    /// The mirror rows to remove once the run is over and its re-fetch is `diff` (built from
    /// [`SeriesPlan::expect_after`] with the same `skipped`). A split's new series must show first, so
    /// a list that lags leaves the original's rows on screen rather than a gap (DS18, A14); anything
    /// else removes whatever the re-fetch shows. The rows of occurrences the run couldn't cancel stay
    /// (R2, R5).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "first caller lands in C9, editing repeating events"
        )
    )]
    pub fn removes_for(&self, diff: &SeriesDiff, skipped: &[usize]) -> Removes {
        let splits = self.steps.iter().any(|s| matches!(s.op, Op::Insert { .. }));
        if splits && diff.stale {
            return Removes::Nothing;
        }
        match self.removes.clone() {
            Removes::From { series, at, .. } => Removes::From {
                series,
                at,
                left: self.left_behind(skipped),
            },
            other => other,
        }
    }

    /// Where the cleanup cancels among `skipped` would have cancelled.
    fn left_behind(&self, skipped: &[usize]) -> Vec<Point> {
        skipped
            .iter()
            .filter_map(|i| self.steps.get(*i))
            .filter(|s| s.stage == Stage::Cleanup)
            .filter_map(|s| match s.op {
                Op::Cancel { original, .. } => Some(original),
                _ => None,
            })
            .collect()
    }

    /// Whether two values of `key` say the same: start and end as instants in a zone, a rule by what it
    /// means, guests by who they are, Google's defaults filled in, anything else by value. Never
    /// text-only, which reads any two lists as equal (DS9, G4).
    fn same(&self, key: &str, a: Option<&Value>, b: Option<&Value>) -> bool {
        let or = |v: Option<&Value>, d: &str| v.and_then(Value::as_str).unwrap_or(d).to_string();
        match key {
            "start" | "end" => when_eq(a, b),
            // A guest's reply isn't a change to the guest list (C8b review EXEC-6).
            "attendees" => attendee_set(a) == attendee_set(b),
            "recurrence" => same_recurrence(&lines_of(a), &lines_of(b), &self.start),
            "status" => or(a, "confirmed") == or(b, "confirmed"),
            "transparency" => or(a, "opaque") == or(b, "opaque"),
            "visibility" => or(a, "default") == or(b, "default"),
            "summary" | "description" | "location" | "colorId" => or(a, "") == or(b, ""),
            "reminders" => {
                let d = json!({ "useDefault": true });
                a.filter(|v| !v.is_null()).unwrap_or(&d) == b.filter(|v| !v.is_null()).unwrap_or(&d)
            }
            _ => a.filter(|v| !v.is_null()) == b.filter(|v| !v.is_null()),
        }
    }
}

/// The checks every series plan starts with: both copies there, the series' own facts, the
/// occurrence one of its own.
fn prepare(input: &SeriesInput) -> Result<(Master, Point), NotPlanned> {
    let (instance, master_json) = (input.fresh.instance.json(), input.fresh.master.json());
    if is_gone(instance) || is_gone(master_json) {
        return Err(NotPlanned::Gone);
    }
    let master = Master::read(input.fresh.master)?;
    if let Some(reason) = master.read_only() {
        return Err(NotPlanned::ReadOnly(reason));
    }
    if master.guests && !input.guests_allowed {
        return Err(NotPlanned::ReadOnly(ReadOnlyReason::HasGuests));
    }
    if instance.get("recurringEventId").and_then(Value::as_str) != Some(master.id.as_str()) {
        return Err(SeriesRefusal::NotInSeries.into());
    }
    if !instance
        .get("id")
        .and_then(Value::as_str)
        .is_some_and(is_plain_segment)
        || instance.get("etag").and_then(Value::as_str).is_none()
        || !is_plain_segment(input.calendar)
    {
        return Err(SeriesRefusal::Unreadable.into());
    }
    let original = original_of(instance).ok_or(SeriesRefusal::Unreadable)?;
    Ok((master, original))
}

fn text(v: &Value, key: &str) -> String {
    v.get(key).and_then(Value::as_str).unwrap_or("").to_string()
}

/// A `recurrence` value's lines.
fn lines_of(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// The values `fresh` holds for `keys`, absent ones as `null`.
fn snapshot(fresh: &Value, keys: &[&str]) -> Map<String, Value> {
    keys.iter()
        .map(|k| {
            (
                (*k).to_string(),
                fresh.get(*k).cloned().unwrap_or(Value::Null),
            )
        })
        .collect()
}

/// A cut needs the series' count of what came before (A7) when its rule has a COUNT.
fn before_split(
    master: &Master,
    input: &SeriesInput,
    at: Point,
) -> Result<Option<u32>, NotPlanned> {
    let parsed = lines::parse(&master.recurrence);
    let (_, facts) = lines::series_rule(&parsed).map_err(RecurError::TextOnly)?;
    if facts.count.is_none() {
        return Ok(None);
    }
    // RFC 5545 counts a start its rule doesn't make, and the walk doesn't: Google's count and PM's
    // would then agree on everything but the one occurrence that decides where the series ends (C8b
    // review ADV-4).
    if !is_occurrence(&facts, &master.start, master.first)? {
        return Err(RecurError::CannotCount.into());
    }
    let listing = input.instances.ok_or(RecurError::CannotCount)?;
    Ok(Some(google_before(master, listing, at)?))
}

/// The series' own stored exceptions: a listing by iCal UID also holds the master, and any other
/// series split off it (C8b review G6).
fn own_exceptions<'a>(
    master: &Master,
    exceptions: &'a Exceptions,
) -> impl Iterator<Item = &'a Value> {
    let id = master.id.clone();
    exceptions
        .items
        .iter()
        .filter(move |i| i.get("recurringEventId").and_then(Value::as_str) == Some(id.as_str()))
}

/// A series cut just before one of its occurrences.
struct SplitCut {
    /// The original's new `recurrence`.
    cut: Vec<String>,
    /// Its EXDATEs and RDATEs from the occurrence on, for the rest.
    moved: Vec<lines::Line>,
    /// Google's count of what came before, when the rule has a COUNT.
    before: Option<u32>,
}

/// The original cut just before `at`, for "this and following": `None` when `at` is the series'
/// first occurrence (the whole series).
fn cut_at(master: &Master, input: &SeriesInput, at: Point) -> Result<Option<SplitCut>, NotPlanned> {
    let parsed = lines::parse(&master.recurrence);
    let (_, facts) = lines::series_rule(&parsed).map_err(RecurError::TextOnly)?;
    // An extra date (RDATE) isn't a place a rule can be cut.
    if !is_occurrence(&facts, &master.start, at)? {
        return Err(RetimeError::NotOnRule.into());
    }
    let k = before_split(master, input, at)?;
    match truncate_recurrence(&master.recurrence, &master.start, at, k)? {
        Truncation::First => Ok(None),
        Truncation::Cut(cut) => Ok(Some(SplitCut {
            cut: cut.lines,
            moved: cut.moved,
            before: k,
        })),
    }
}

/// Exceptions on or after `at`, changed or cancelled, from a complete listing.
fn exceptions_from<'a>(
    master: &Master,
    exceptions: &'a Exceptions,
    at: Point,
) -> Result<Vec<(&'a Value, Point)>, NotPlanned> {
    if !exceptions.complete {
        return Err(SeriesRefusal::Incomplete.into());
    }
    let zone = master.start.zone();
    let mut out = Vec::new();
    for item in own_exceptions(master, exceptions) {
        let p = original_of(item).ok_or(SeriesRefusal::Unreadable)?;
        if !p.cmp_in(&at, zone).is_lt() {
            out.push((item, p));
        }
    }
    Ok(out)
}

/// A cleanup cancel of the original's own copy of the occurrence at `original`, which moved to the new
/// series (or was cut off): done when Google already shows it cancelled or gone; re-sent on a 412
/// only while nothing it would throw away has changed (DS10).
fn cancel_exception(
    calendar: &str,
    item: &Value,
    original: Point,
    notify: Notify,
    requires: Option<usize>,
) -> Result<Step, NotPlanned> {
    let event = item
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| is_plain_segment(id))
        .ok_or(SeriesRefusal::Unreadable)?;
    let etag = item
        .get("etag")
        .and_then(Value::as_str)
        .ok_or(SeriesRefusal::Unreadable)?;
    Ok(Step {
        op: Op::Cancel {
            calendar: calendar.to_string(),
            event: event.to_string(),
            etag: etag.to_string(),
            original,
        },
        notify,
        stage: Stage::Cleanup,
        requires,
        conflict: Recheck::Unchanged(snapshot(item, CANCEL_WATCH)),
        shows: Shows::Fields {
            sent: snapshot(&json!({ "status": "cancelled" }), &["status"]),
            old: snapshot(item, &["status"]),
        },
        gone_is_done: true,
        undo: None,
    })
}

/// The steps that delete "this and following" (no new series): the cut, then the original's own
/// stored copies of the occurrences from it on, which would otherwise outlive it (INFERRED; C9a's
/// L-R2/L-R5 probe what Google does with them). Every stored one that isn't cancelled already, as a
/// split does: `Exceptions` holds stored exceptions only, never every occurrence.
fn plan_cut_delete(
    master: &Master,
    input: &SeriesInput,
    at: Point,
    cut: Vec<String>,
    notify: Notify,
) -> Result<Vec<Step>, NotPlanned> {
    let master_json = input.fresh.master.json();
    let mut steps = vec![cut_step(
        master,
        input.calendar,
        master_json,
        cut,
        &["recurrence", "start", "end"],
        notify,
    )];
    for (item, p) in exceptions_from(master, input.exceptions, at)? {
        if classify_instance(master_json, master, item) != InstanceKind::Cancelled {
            steps.push(cancel_exception(input.calendar, item, p, notify, None)?);
        }
    }
    Ok(steps)
}

/// The PATCH that cuts the original. On a 412 it's sent again only if `watched` is as the plan saw
/// it: its rule and times, and for a split every field the new series copied (AS10), or the new
/// series would carry a value someone has just changed.
fn cut_step(
    master: &Master,
    calendar: &str,
    master_json: &Value,
    cut: Vec<String>,
    watched: &[&str],
    notify: Notify,
) -> Step {
    Step {
        op: Op::Patch {
            calendar: calendar.to_string(),
            event: master.id.clone(),
            etag: master.etag.clone(),
            body: [("recurrence".to_string(), Value::from(cut.clone()))]
                .into_iter()
                .collect(),
        },
        notify,
        stage: Stage::Commit,
        requires: None,
        conflict: Recheck::Unchanged(snapshot(master_json, watched)),
        shows: Shows::Fields {
            sent: [("recurrence".to_string(), Value::from(cut))]
                .into_iter()
                .collect(),
            old: snapshot(master_json, &["recurrence"]),
        },
        gone_is_done: false,
        undo: None,
    }
}

/// Delete the occurrence (`This`), the series from it on (`Following`), or the whole series (`All`)
/// (T8). `seen` is what the user saw of the occurrence; the delete goes ahead only if that's still
/// what's there (A8, A15).
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "first caller lands in C9, editing repeating events"
    )
)]
pub fn plan_series_delete(
    scope: Scope,
    input: &SeriesInput,
    seen: &SeenSummary,
    notify: Notify,
) -> Result<SeriesPlan, NotPlanned> {
    let (master, original) = prepare(input)?;
    let instance = input.fresh.instance.json();
    match delete_still_matches(seen, instance) {
        DeleteCheck::Matches => {}
        DeleteCheck::Gone => return Err(NotPlanned::Gone),
        DeleteCheck::Changed(fields) => return Err(NotPlanned::Conflict(fields)),
    }
    let master_json = input.fresh.master.json();
    let mut warnings = Vec::new();
    if input.sibling && scope != Scope::This {
        warnings.push(SeriesWarning::SplitSibling);
    }
    let whole = |warnings: Vec<SeriesWarning>| SeriesPlan {
        scope: Scope::All,
        steps: vec![Step {
            op: Op::Delete {
                calendar: input.calendar.to_string(),
                event: master.id.clone(),
                etag: master.etag.clone(),
            },
            notify,
            stage: Stage::Commit,
            requires: None,
            conflict: Recheck::Series {
                summary: master_json.get("summary").cloned().unwrap_or(Value::Null),
                start: master_json.get("start").cloned().unwrap_or(Value::Null),
                recurrence: master.recurrence.clone(),
            },
            shows: Shows::Gone,
            gone_is_done: true,
            undo: None,
        }],
        warnings,
        expect: vec![Expect::Absent {
            series: master.id.clone(),
        }],
        removes: Removes::Series {
            series: master.id.clone(),
        },
        start: master.start,
    };
    match scope {
        Scope::This => Ok(SeriesPlan {
            scope,
            steps: vec![Step {
                op: Op::Cancel {
                    calendar: input.calendar.to_string(),
                    event: text(instance, "id"),
                    etag: text(instance, "etag"),
                    original,
                },
                notify,
                stage: Stage::Commit,
                requires: None,
                conflict: Recheck::SeenStillMatches(seen.clone()),
                shows: Shows::Fields {
                    sent: snapshot(&json!({ "status": "cancelled" }), &["status"]),
                    old: snapshot(instance, &["status"]),
                },
                gone_is_done: true,
                undo: None,
            }],
            warnings,
            expect: vec![Expect::Cancelled {
                series: master.id.clone(),
                original,
            }],
            removes: Removes::Occurrence {
                event: text(instance, "id"),
            },
            start: master.start,
        }),
        Scope::All => Ok(whole(warnings)),
        Scope::Following if is_first(&master, input.fresh.instance) => Ok(whole(warnings)),
        Scope::Following => match cut_at(&master, input, original)? {
            None => Ok(whole(warnings)),
            Some(SplitCut { cut, .. }) => Ok(SeriesPlan {
                scope,
                steps: plan_cut_delete(&master, input, original, cut, notify)?,
                warnings,
                expect: vec![Expect::EndsBefore {
                    series: master.id.clone(),
                    at: original,
                    left: Vec::new(),
                }],
                removes: Removes::From {
                    series: master.id.clone(),
                    at: original,
                    left: Vec::new(),
                },
                start: master.start,
            }),
        },
    }
}

/// Where the draft puts the occurrence, when it changes its time: its new start, end and length.
struct NewTime {
    start: Point,
    length: Length,
}

/// The draft's time, if it changes the occurrence's. A zone the user picked for its start or its
/// end that isn't the occurrence's own can't be carried to the series (until C11).
fn new_time(draft: &EventPatchDraft, instance: &Value) -> Result<Option<NewTime>, NotPlanned> {
    let Some(draft_time) = &draft.time else {
        return Ok(None);
    };
    let when = time::resolve_against(draft_time, instance.get("start"), instance.get("end"))?;
    if when_eq(instance.get("start"), Some(&when.start))
        && when_eq(instance.get("end"), Some(&when.end))
    {
        return Ok(None);
    }
    if zones_changed(draft_time, instance) {
        return Err(SeriesRefusal::ZoneChange.into());
    }
    let (start, end) = (point_of(Some(&when.start)), point_of(Some(&when.end)));
    let length = match (start, end) {
        (Some(Point::At(s)), Some(Point::At(e))) => Length::Timed(e - s),
        (Some(Point::Day(s)), Some(Point::Day(e))) => Length::AllDay((e - s).num_days()),
        _ => return Err(SeriesRefusal::Unreadable.into()),
    };
    Ok(Some(NewTime {
        start: start.ok_or(SeriesRefusal::Unreadable)?,
        length,
    }))
}

/// Google's `start` and `end` for a series starting at `first` with occurrences `length` long, in
/// the master's own zones.
fn when_nodes(master: &Master, first: Point, length: Length) -> Result<(Value, Value), NotPlanned> {
    let zone = master.start.zone();
    let timed = |t: DateTime<Utc>, zone: Tz| {
        json!({
            "dateTime": t.with_timezone(&zone).to_rfc3339_opts(SecondsFormat::Secs, false),
            "timeZone": zone.name(),
        })
    };
    let date = |d: NaiveDate| json!({ "date": d.format("%Y-%m-%d").to_string() });
    match (first, length) {
        (Point::At(t), Length::Timed(len)) => Ok((timed(t, zone), timed(t + len, master.end_zone))),
        (Point::Day(d), Length::AllDay(days)) => {
            let end = d
                .checked_add_signed(chrono::TimeDelta::days(days))
                .ok_or(SeriesRefusal::Unreadable)?;
            Ok((date(d), date(end)))
        }
        _ => Err(RetimeError::KindChange.into()),
    }
}

/// EXDATE points already in `lines`.
fn excluded(lines_now: &[String], start: &SeriesStart) -> Vec<Point> {
    lines::parse(lines_now)
        .iter()
        .filter_map(|l| match &l.kind {
            Kind::ExDate(dates) => Some(
                dates
                    .values
                    .iter()
                    .filter_map(|v| dates.point(v, start))
                    .collect::<Vec<_>>(),
            ),
            _ => None,
        })
        .flatten()
        .collect()
}

/// `lines` with an EXDATE added for each cancelled occurrence in `cancelled` (where each now is),
/// those already excluded left out. A cancelled occurrence that has no place in the moved series is
/// refused by `moved`, never dropped (RM1).
fn with_cancelled(
    lines_now: Vec<String>,
    start: &SeriesStart,
    cancelled: &[Point],
    retimed: Option<&Retimed>,
) -> Result<Vec<String>, NotPlanned> {
    let already = excluded(&lines_now, start);
    let mut add = Vec::new();
    for p in cancelled {
        let now = match retimed {
            Some(r) => r.moved(*p)?,
            None => Some(*p),
        };
        if let Some(now) = now.filter(|q| !already.contains(q)) {
            add.push(now);
        }
    }
    let mut out = lines_now;
    out.extend(exdate_lines(&add).into_iter().map(|l| l.raw));
    Ok(out)
}

/// The draft's touched text fields, as Google names them.
fn draft_fields(draft: &EventPatchDraft) -> Vec<(&'static str, Value)> {
    let mut out = Vec::new();
    if let Some(v) = &draft.summary {
        out.push(("summary", Value::from(v.clone())));
    }
    if let Some(v) = &draft.location {
        out.push(("location", Value::from(v.clone())));
    }
    if let Some(v) = &draft.description {
        out.push(("description", Value::from(v.clone())));
    }
    if let Some(v) = draft.show_as {
        out.push(("transparency", Value::from(v.as_transparency())));
    }
    if let Some(v) = draft.visibility {
        out.push(("visibility", Value::from(v.as_param())));
    }
    out
}

/// An empty plan, for comparing values the way a plan does.
fn probe(start: &SeriesStart) -> SeriesPlan {
    SeriesPlan {
        scope: Scope::All,
        steps: Vec::new(),
        warnings: Vec::new(),
        expect: Vec::new(),
        removes: Removes::Nothing,
        start: *start,
    }
}

/// The opened copy against the fresh one on `keys`: the fields changed in Google meanwhile (R2).
fn changed_since_opened(
    plan_start: &SeriesStart,
    opened: &Value,
    fresh: &Value,
    keys: &[String],
) -> Vec<String> {
    let probe = probe(plan_start);
    keys.iter()
        .filter(|k| !probe.same(k, opened.get(k.as_str()), fresh.get(k.as_str())))
        .cloned()
        .collect()
}

/// Change the occurrence (`This`), the series from it on (`Following`, a split), or the whole series
/// (`All`) as `draft` says. `tail_id` is the id a split's new series is inserted under: C9 mints a new
/// one with `plan::client_event_id` for every new plan, and reuses it only to send this same plan
/// again (the module's "one plan, one tail id").
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "first caller lands in C9, editing repeating events"
    )
)]
pub fn plan_series_edit(
    scope: Scope,
    input: &SeriesInput,
    draft: &EventPatchDraft,
    tail_id: &str,
    notify: Notify,
) -> Result<SeriesPlan, NotPlanned> {
    let (master, original) = prepare(input)?;
    match scope {
        Scope::This => plan_this(input, &master, draft, notify),
        Scope::All => plan_all(input, &master, original, draft, notify),
        Scope::Following if is_first(&master, input.fresh.instance) => {
            plan_all(input, &master, original, draft, notify)
        }
        Scope::Following => match cut_at(&master, input, original)? {
            None => plan_all(input, &master, original, draft, notify),
            Some(cut) => plan_split(input, &master, original, cut, draft, tail_id, notify),
        },
    }
}

fn plan_this(
    input: &SeriesInput,
    master: &Master,
    draft: &EventPatchDraft,
    notify: Notify,
) -> Result<SeriesPlan, NotPlanned> {
    let instance = input.fresh.instance.json();
    let body = build_patch(instance, draft)?;
    if body.is_empty() {
        return Err(NotPlanned::NoChange);
    }
    if body.contains_key("visibility") {
        return Err(SeriesRefusal::VisibilityIsSeriesWide.into());
    }
    match update_check(input.opened.instance.json(), instance, &body) {
        UpdateCheck::Proceed => {}
        UpdateCheck::Gone => return Err(NotPlanned::Gone),
        UpdateCheck::Conflict(fields) => return Err(NotPlanned::Conflict(fields)),
    }
    let keys: Vec<&str> = body.keys().map(String::as_str).collect();
    let before = snapshot(instance, &keys);
    Ok(SeriesPlan {
        scope: Scope::This,
        steps: vec![Step {
            op: Op::Patch {
                calendar: input.calendar.to_string(),
                event: text(instance, "id"),
                etag: text(instance, "etag"),
                body: body.clone(),
            },
            notify,
            stage: Stage::Commit,
            requires: None,
            conflict: Recheck::Unchanged(before.clone()),
            shows: Shows::Fields {
                sent: body,
                old: before,
            },
            gone_is_done: false,
            undo: None,
        }],
        warnings: Vec::new(),
        expect: Vec::new(),
        removes: Removes::Nothing,
        start: master.start,
    })
}

fn plan_all(
    input: &SeriesInput,
    master: &Master,
    original: Point,
    draft: &EventPatchDraft,
    notify: Notify,
) -> Result<SeriesPlan, NotPlanned> {
    let (instance, master_json) = (input.fresh.instance.json(), input.fresh.master.json());
    let opened = input.opened.master.ok_or(SeriesRefusal::NoBase)?;
    let text_only = EventPatchDraft {
        time: None,
        ..draft.clone()
    };
    let mut body = build_patch(master_json, &text_only)?;
    let mut warnings = Vec::new();
    let mut undo = None;
    let zone = master.start.zone();
    if draft.time.is_some() {
        // The draft's time is read against the occurrence as Google holds it now, so it must still be
        // where the user saw it (R2; C8b reviews ADV-1).
        unchanged_since_opened(master, input, &["start", "end"])?;
    }
    if let Some(new) = new_time(draft, instance)? {
        let retimed = retime(
            &master.recurrence,
            &master.start,
            master.first,
            original,
            new.start,
            None,
        )?;
        // Whether the series' starts move. An occurrence moved on its own and put back on its
        // series' slot moves nothing: only this occurrence differs, and saving the rest of the draft
        // without its time would drop part of it (C8b reviews ADV-2).
        let starts_move = retimed.remap.day != DayMap::Same || retimed.first != master.first;
        // An occurrence with a time of its own keeps it whatever the series' new length is, so a draft
        // that doesn't move the series can't move it either (C8b fix review).
        let own_time = matches!(
            classify_instance(master_json, master, instance),
            InstanceKind::Modified { carry, .. } if carry.contains(&"time")
        );
        if !starts_move && (new.length == master.length || own_time) {
            return Err(if body.is_empty() && new.length == master.length {
                SeriesRefusal::OnlyThisDiffers
            } else {
                SeriesRefusal::OnlyThisTimeDiffers
            }
            .into());
        }
        // A new length alone leaves the rule and its exceptions as they are.
        let recurrence = if starts_move {
            let cancelled = cancelled_points(master, input.exceptions)?;
            with_cancelled(
                retimed.lines.clone(),
                &retimed.start,
                &cancelled,
                Some(&retimed),
            )?
        } else {
            master.recurrence.clone()
        };
        let (start, end) = when_nodes(master, retimed.first, new.length)?;
        if retimed.remap.day != DayMap::Same {
            warnings.push(SeriesWarning::RepeatDayChanges {
                from: original.day_in(zone),
                to: new.start.day_in(zone),
            });
        }
        if retimed.first.day_in(zone) != master.first.day_in(zone) {
            warnings.push(SeriesWarning::StartMoved {
                from: master.first.day_in(zone),
                to: retimed.first.day_in(zone),
            });
        }
        if new.length != master.length {
            warnings.push(SeriesWarning::LengthChanges);
        }
        if !when_eq(master_json.get("start"), Some(&start)) {
            body.insert("start".into(), start);
        }
        if !when_eq(master_json.get("end"), Some(&end)) {
            body.insert("end".into(), end);
        }
        if !same_recurrence(&recurrence, &master.recurrence, &master.start) {
            body.insert("recurrence".into(), Value::from(recurrence));
        }
        if starts_move {
            undo = Some(Undo::RestoreMaster {
                calendar: input.calendar.to_string(),
                event: master.id.clone(),
                body: snapshot(master_json, &["start", "end", "recurrence"]),
            });
        }
    }
    if body.is_empty() {
        let this_only = build_patch(instance, draft)?;
        return Err(if this_only.is_empty() {
            NotPlanned::NoChange
        } else {
            SeriesRefusal::OnlyThisDiffers.into()
        });
    }
    // A new end is measured from the series' start, so it watches the start and rule too.
    let mut watched: Vec<String> = body.keys().cloned().collect();
    if ["start", "end", "recurrence"]
        .iter()
        .any(|k| body.contains_key(*k))
    {
        for k in ["start", "end", "recurrence"] {
            if !watched.iter().any(|w| w == k) {
                watched.push(k.to_string());
            }
        }
    }
    let changed = changed_since_opened(&master.start, opened.json(), master_json, &watched);
    if !changed.is_empty() {
        return Err(NotPlanned::Conflict(changed));
    }
    // A new time can leave every occurrence changed on its own behind (its place in the series no
    // longer matches, RFC 5545); a text change only those that changed the same field.
    let retimed = body.contains_key("start") || body.contains_key("recurrence");
    let reset = own_exceptions(master, input.exceptions)
        .filter(|item| {
            matches!(
                classify_instance(master_json, master, item),
                InstanceKind::Modified { carry, other }
                    if retimed || carry.iter().chain(&other).any(|c| body.contains_key(*c))
            )
        })
        .count();
    if reset > 0 {
        warnings.push(SeriesWarning::ExceptionsMayReset(
            u32::try_from(reset).unwrap_or(u32::MAX),
        ));
    }
    let watched: Vec<&str> = watched.iter().map(String::as_str).collect();
    let keys: Vec<&str> = body.keys().map(String::as_str).collect();
    let old = snapshot(master_json, &keys);
    Ok(SeriesPlan {
        scope: Scope::All,
        steps: vec![Step {
            op: Op::Patch {
                calendar: input.calendar.to_string(),
                event: master.id.clone(),
                etag: master.etag.clone(),
                body: body.clone(),
            },
            notify,
            stage: Stage::Commit,
            requires: None,
            conflict: Recheck::Unchanged(snapshot(master_json, &watched)),
            shows: Shows::Fields { sent: body, old },
            gone_is_done: false,
            undo,
        }],
        warnings,
        expect: Vec::new(),
        removes: Removes::Nothing,
        start: master.start,
    })
}

/// The original starts of the series' cancelled occurrences, from a complete listing.
fn cancelled_points(master: &Master, exceptions: &Exceptions) -> Result<Vec<Point>, NotPlanned> {
    if !exceptions.complete {
        return Err(SeriesRefusal::Incomplete.into());
    }
    Ok(own_exceptions(master, exceptions)
        .filter(|i| i.get("status").and_then(Value::as_str) == Some("cancelled"))
        .filter_map(original_of)
        .collect())
}

/// A conflict when the occurrence no longer holds, on `keys`, what the user opened (R2).
fn unchanged_since_opened(
    master: &Master,
    input: &SeriesInput,
    keys: &[&str],
) -> Result<(), NotPlanned> {
    let keys: Vec<String> = keys.iter().map(|k| (*k).to_string()).collect();
    let changed = changed_since_opened(
        &master.start,
        input.opened.instance.json(),
        input.fresh.instance.json(),
        &keys,
    );
    if changed.is_empty() {
        Ok(())
    } else {
        Err(NotPlanned::Conflict(changed))
    }
}

fn plan_split(
    input: &SeriesInput,
    master: &Master,
    at: Point,
    SplitCut { cut, moved, before }: SplitCut,
    draft: &EventPatchDraft,
    tail_id: &str,
    notify: Notify,
) -> Result<SeriesPlan, NotPlanned> {
    let (instance, master_json) = (input.fresh.instance.json(), input.fresh.master.json());
    let opened = input.opened.master.ok_or(SeriesRefusal::NoBase)?;
    // An id PM minted for this plan, never the series' own (C8b review ADV-5).
    if !is_plain_segment(tail_id) || tail_id == master.id {
        return Err(SeriesRefusal::Unreadable.into());
    }
    let parsed = lines::parse(&master.recurrence);
    let (_, facts) = lines::series_rule(&parsed).map_err(RecurError::TextOnly)?;
    let remaining = match (facts.count, before) {
        (Some(count), Some(k)) => Some(remaining_count(count, k)?),
        _ => None,
    };
    let rest = tail_recurrence(&master.recurrence, remaining, &moved)?;
    let zone = master.start.zone();
    // The rest repeats at the series' own wall time from the split's day. On a day the clocks skip
    // that time, the occurrence sits an hour off it, and the rest can't start there: it would repeat
    // an hour late, and every EXDATE after it would name nothing (C8b fix review).
    let rest_start = match (master.start, at) {
        (SeriesStart::Timed { local, .. }, Point::At(_)) => {
            // The occurrence's own day: where the series' time, read as RFC 5545 reads a skipped
            // hour, lands on `at`. A time in an evening's skipped hour lands on the next date.
            let time = local.time();
            let today = at.day_in(zone);
            let day = [today, today.pred_opt().unwrap_or(today)]
                .into_iter()
                .find(|d| rfc_instant(zone, d.and_time(time)).map(Point::At) == Some(at))
                .unwrap_or(today);
            let start = SeriesStart::Timed {
                local: day.and_time(time),
                zone,
            };
            if start.point() != Some(at) {
                return Err(SeriesRefusal::SplitInGap(day).into());
            }
            start
        }
        (SeriesStart::AllDay(_), Point::Day(d)) => SeriesStart::AllDay(d),
        _ => return Err(RetimeError::KindChange.into()),
    };
    let exceptions = exceptions_from(master, input.exceptions, at)?;
    let cancelled: Vec<Point> = exceptions
        .iter()
        .filter(|(item, _)| classify_instance(master_json, master, item) == InstanceKind::Cancelled)
        .map(|(_, p)| *p)
        .collect();

    let mut warnings = Vec::new();
    if draft.time.is_some() {
        // The draft's time is read against the occurrence as Google holds it now: it must still be
        // where the user saw it, or a move made in Google would be overwritten (R2; C8b reviews
        // ADV-1).
        unchanged_since_opened(master, input, &["start", "end"])?;
    }
    let new = new_time(draft, instance)?;
    let (tail_lines, first, length, retimed) = match &new {
        Some(new) => {
            let deleted = cancelled_points(master, input.exceptions)?;
            let head = last_kept(master, &facts, at, &deleted)?;
            let retimed = retime(&rest, &rest_start, at, at, new.start, head)?;
            let lines = with_cancelled(
                retimed.lines.clone(),
                &retimed.start,
                &cancelled,
                Some(&retimed),
            )?;
            if retimed.remap.day != DayMap::Same {
                warnings.push(SeriesWarning::RepeatDayChanges {
                    from: at.day_in(zone),
                    to: new.start.day_in(zone),
                });
            }
            (lines, retimed.first, new.length, Some(retimed))
        }
        None => (
            with_cancelled(rest, &rest_start, &cancelled, None)?,
            at,
            master.length,
            None,
        ),
    };
    if length != master.length {
        warnings.push(SeriesWarning::LengthChanges);
    }
    // Whether the rest of the series changes at all: a draft Google already holds, or one that only
    // puts this occurrence back on its series' slot, is no reason for a new series (C8b review ADV-3).
    let moves = retimed
        .as_ref()
        .is_some_and(|r| r.remap.day != DayMap::Same || r.first != at)
        || length != master.length;
    let touched = draft_fields(draft);
    let compare = probe(&master.start);
    if !moves
        && touched
            .iter()
            .all(|(k, v)| compare.same(k, Some(v), master_json.get(*k)))
    {
        return Err(if build_patch(instance, draft)?.is_empty() {
            NotPlanned::NoChange
        } else {
            SeriesRefusal::OnlyThisDiffers.into()
        });
    }

    // The new series: the original's own fields (from Google's copy, never the mirror), the draft's
    // over them, its times in the original's zones, and a private note of where it came from.
    let mut body: Map<String, Value> = TAIL_COPY
        .iter()
        .filter_map(|k| master_json.get(*k).map(|v| ((*k).to_string(), v.clone())))
        .collect();
    if let Some(guests) = master_json.get("attendees").and_then(Value::as_array) {
        let guests: Vec<Value> = guests
            .iter()
            .filter_map(|g| {
                let mut kept = Map::new();
                for k in ["email", "optional", "displayName"] {
                    if let Some(v) = g.get(k) {
                        kept.insert(k.to_string(), v.clone());
                    }
                }
                kept.contains_key("email").then_some(Value::Object(kept))
            })
            .collect();
        body.insert("attendees".into(), Value::from(guests));
    }
    let mut properties = master_json
        .get("extendedProperties")
        .cloned()
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({}));
    if let Some(obj) = properties.as_object_mut() {
        let private = obj.entry("private").or_insert_with(|| json!({}));
        if let Some(private) = private.as_object_mut() {
            private.insert("pmSplitOf".into(), Value::from(master.id.clone()));
        }
    }
    body.insert("extendedProperties".into(), properties);
    for (k, v) in &touched {
        body.insert((*k).to_string(), v.clone());
    }
    let (start, end) = when_nodes(master, first, length)?;
    body.insert("start".into(), start);
    body.insert("end".into(), end);
    body.insert("recurrence".into(), Value::from(tail_lines));
    // And a note of this plan, so its insert can't be read off another plan's tail (Shows::Tail).
    let plan = plan_digest(&body);
    if let Some(private) = body
        .get_mut("extendedProperties")
        .and_then(|p| p.get_mut("private"))
        .and_then(Value::as_object_mut)
    {
        private.insert("pmSplitPlan".into(), Value::from(plan.clone()));
    }
    warnings.push(SeriesWarning::NotCarried {
        attachments: master_json
            .get("attachments")
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty()),
        conference: master_json.get("conferenceData").is_some()
            || master_json.get("hangoutLink").is_some(),
        label: master_json.get("eventLabelId").is_some(),
    });
    warnings.retain(|w| {
        !matches!(
            w,
            SeriesWarning::NotCarried {
                attachments: false,
                conference: false,
                label: false
            }
        )
    });
    warnings.push(SeriesWarning::NewIdentity);
    if input.sibling {
        warnings.push(SeriesWarning::SplitSibling);
    }

    // Nothing the plan sends may have changed in Google since the editor opened (R2).
    let watched: Vec<String> = TAIL_WATCH.iter().map(|k| (*k).to_string()).collect();
    let changed = changed_since_opened(&master.start, opened.json(), master_json, &watched);
    if !changed.is_empty() {
        return Err(NotPlanned::Conflict(changed));
    }
    let instance_keys: Vec<String> = touched.iter().map(|(k, _)| (*k).to_string()).collect();
    let changed = changed_since_opened(
        &master.start,
        input.opened.instance.json(),
        instance,
        &instance_keys,
    );
    if !changed.is_empty() {
        return Err(NotPlanned::Conflict(changed));
    }

    let mut steps = vec![
        Step {
            op: Op::Insert {
                calendar: input.calendar.to_string(),
                id: tail_id.to_string(),
                body,
            },
            notify,
            stage: Stage::Commit,
            requires: None,
            conflict: Recheck::Never,
            shows: Shows::Tail {
                of: master.id.clone(),
                plan: plan.clone(),
            },
            gone_is_done: false,
            undo: Some(Undo::DeleteInserted {
                calendar: input.calendar.to_string(),
                event: tail_id.to_string(),
                split_of: master.id.clone(),
                plan,
            }),
        },
        cut_step(master, input.calendar, master_json, cut, TAIL_WATCH, notify),
    ];

    // Each stored exception from the split on: what changed on its own onto the new series' same
    // occurrence (Q3), then its old copy cancelled on the original, only once the carry is done
    // (DS3). One with nothing to carry (a guest's reply, a move put back) is cancelled all the same,
    // as a delete of the rest cancels it, or it would outlive the cut (C8b review R1).
    let draft_keys: Vec<&str> = touched.iter().map(|(k, _)| *k).collect();
    let all_day = matches!(master.length, Length::AllDay(_));
    for (item, p) in &exceptions {
        let carry = match classify_instance(master_json, master, item) {
            InstanceKind::Cancelled => continue,
            InstanceKind::Plain => Vec::new(),
            InstanceKind::Modified { other, .. } if !other.is_empty() => {
                return Err(SeriesRefusal::CannotCarry(p.day_in(zone)).into());
            }
            InstanceKind::Modified { carry, .. } => carry,
        };
        let split_itself = *p == at;
        let mut carried = Map::new();
        for field in carry {
            if split_itself && draft_keys.contains(&field) {
                continue;
            }
            if field == "time" {
                if split_itself && new.is_some() {
                    continue;
                }
                for k in ["start", "end"] {
                    let mut node = item.get(k).cloned().unwrap_or(Value::Null);
                    // Timed onto an all-day occurrence or back: Google merges the node, so the
                    // other kind's keys go as nulls (C8b review G4).
                    if node.get("date").is_some() != all_day {
                        drop_other_kind(&mut node);
                    }
                    carried.insert(k.to_string(), node);
                }
                continue;
            }
            let value = item.get(field).cloned().unwrap_or(match field {
                "summary" | "description" | "location" => Value::from(""),
                _ => Value::Null,
            });
            carried.insert(field.to_string(), value);
        }
        let requires = if carried.is_empty() {
            None
        } else {
            let target = match &retimed {
                Some(r) => r.moved(*p)?.ok_or(RetimeError::Loses(p.day_in(zone)))?,
                None => *p,
            };
            steps.push(Step {
                op: Op::Carry {
                    calendar: input.calendar.to_string(),
                    series: tail_id.to_string(),
                    original: target,
                    body: carried.clone(),
                },
                notify,
                stage: Stage::Cleanup,
                requires: None,
                conflict: Recheck::Never,
                shows: Shows::Fields {
                    sent: carried,
                    old: Map::new(),
                },
                gone_is_done: false,
                undo: None,
            });
            Some(steps.len() - 1)
        };
        steps.push(cancel_exception(
            input.calendar,
            item,
            *p,
            notify,
            requires,
        )?);
    }

    Ok(SeriesPlan {
        scope: Scope::Following,
        steps,
        warnings,
        expect: vec![
            Expect::EndsBefore {
                series: master.id.clone(),
                at,
                left: Vec::new(),
            },
            Expect::Present {
                series: tail_id.to_string(),
            },
        ],
        removes: Removes::From {
            series: master.id.clone(),
            at,
            left: Vec::new(),
        },
        start: master.start,
    })
}

/// The last occurrence the original keeps before `at`, a split's occurrence: its rule's latest that
/// isn't deleted (an EXDATE, or among `cancelled`), else its start, which RFC 5545 and Google count
/// even off its own rule. The rest can't start on that day or before (C8b reviews ADV-9). An extra
/// date (RDATE) is an occurrence of its own, which the rest's rule can't double, so it doesn't count.
fn last_kept(
    master: &Master,
    facts: &lines::RuleFacts,
    at: Point,
    cancelled: &[Point],
) -> Result<Option<Point>, NotPlanned> {
    let zone = master.start.zone();
    let deleted = excluded(&master.recurrence, &master.start);
    let kept = |p: &Point| !deleted.contains(p) && !cancelled.contains(p);
    let last = last_before(facts, &master.start, at, kept)?;
    Ok(last.or(Some(master.first).filter(|f| f.cmp_in(&at, zone).is_lt() && kept(f))))
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::*;
    use super::*;
    use crate::calendar_write::classify::Method;
    use crate::calendar_write::dto::{ShowAs, TimeDraft};

    const TAIL: &str = "0123456789abcdef0123456789abcdef";

    /// One series as C9 would hand it over: the series, the occurrence picked, and its exceptions.
    struct Case {
        master: FreshEvent,
        instance: FreshEvent,
        opened_instance: FreshEvent,
        opened_master: FreshEvent,
        exceptions: Exceptions,
        instances: Option<Listing>,
        sibling: bool,
        guests_allowed: bool,
    }

    impl Case {
        fn new(master: Value, instance: Value) -> Case {
            Case {
                master: fresh(master.clone()),
                instance: fresh(instance.clone()),
                opened_instance: fresh(instance),
                opened_master: fresh(master),
                exceptions: complete(Vec::new()),
                instances: None,
                sibling: false,
                guests_allowed: false,
            }
        }

        fn exceptions(mut self, items: Vec<Value>) -> Case {
            self.exceptions = complete(items);
            self
        }

        /// As C12 will run it, a series with guests allowed.
        fn guests(mut self) -> Case {
            self.guests_allowed = true;
            self
        }

        fn input(&self) -> SeriesInput<'_> {
            SeriesInput {
                calendar: CALENDAR,
                opened: Opened {
                    instance: &self.opened_instance,
                    master: Some(&self.opened_master),
                },
                fresh: Fresh {
                    instance: &self.instance,
                    master: &self.master,
                },
                exceptions: &self.exceptions,
                instances: self.instances.as_ref(),
                sibling: self.sibling,
                guests_allowed: self.guests_allowed,
            }
        }

        fn delete(&self, scope: Scope) -> Result<SeriesPlan, NotPlanned> {
            plan_series_delete(scope, &self.input(), &seen_of(&self.instance), Notify::None)
        }

        fn edit(&self, scope: Scope, draft: EventPatchDraft) -> Result<SeriesPlan, NotPlanned> {
            plan_series_edit(scope, &self.input(), &draft, TAIL, Notify::None)
        }
    }

    /// What the user saw of an occurrence: its mirror row's summary and times.
    fn seen_of(instance: &FreshEvent) -> SeenSummary {
        let row = crate::calendar::parse_event("", instance.json()).unwrap();
        SeenSummary {
            summary: row.summary,
            start: row.start,
            end: row.end,
            all_day: row.all_day,
            location: row.location,
        }
    }

    fn weekly() -> Value {
        master(&["RRULE:FREQ=WEEKLY;BYDAY=MO"])
    }

    fn monday_19() -> Value {
        plain("2026-10-19T09:00:00+01:00")
    }

    fn title(s: &str) -> EventPatchDraft {
        EventPatchDraft {
            summary: Some(s.into()),
            ..Default::default()
        }
    }

    fn at(time: &str, date: &str) -> EventPatchDraft {
        let end = format!("{:02}:00", time[..2].parse::<u32>().unwrap() + 1);
        EventPatchDraft {
            time: Some(TimeDraft::Timed {
                start_date: date.into(),
                start_time: time.into(),
                start_zone: "Europe/London".into(),
                end_date: date.into(),
                end_time: end,
                end_zone: "Europe/London".into(),
            }),
            ..Default::default()
        }
    }

    fn body(step: &Step) -> &Map<String, Value> {
        step.op.body().unwrap()
    }

    fn point(s: &str) -> Point {
        Point::At(DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc))
    }

    // --- T8: deleting ---

    #[test]
    fn deleting_this_occurrence_cancels_it_on_what_the_user_saw() {
        let case = Case::new(weekly(), monday_19());
        let plan = case.delete(Scope::This).unwrap();
        assert_eq!(plan.scope, Scope::This);
        assert_eq!(plan.steps.len(), 1);
        let step = &plan.steps[0];
        assert_eq!(
            step.op,
            Op::Cancel {
                calendar: CALENDAR.into(),
                event: "abc_20261019T080000Z".into(),
                etag: text(case.instance.json(), "etag"),
                original: point("2026-10-19T08:00:00Z"),
            }
        );
        assert!(matches!(step.conflict, Recheck::SeenStillMatches(_)));
        assert!(step.gone_is_done);
        let request = step.op.request(Notify::None).unwrap();
        assert_eq!(request.method, Method::Patch);
        assert_eq!(request.body, Some(json!({ "status": "cancelled" })));
        // DS15: a reply that kept the occurrence proves it wasn't cancelled.
        assert_eq!(plan.verdict(0, Some(case.instance.json())), Applied::No);
        assert_eq!(
            plan.verdict(0, Some(&cancelled("2026-10-19T09:00:00+01:00"))),
            Applied::Yes
        );
        assert_eq!(plan.verdict(0, None), Applied::Yes);
        assert_eq!(
            plan.removes,
            Removes::Occurrence {
                event: "abc_20261019T080000Z".into()
            }
        );
    }

    #[test]
    fn deleting_all_or_from_the_first_deletes_the_series() {
        let case = Case::new(weekly(), plain("2026-10-05T09:00:00+01:00"));
        for scope in [Scope::All, Scope::Following] {
            let plan = case.delete(scope).unwrap();
            assert_eq!(plan.scope, Scope::All);
            assert_eq!(plan.steps.len(), 1);
            assert_eq!(
                plan.steps[0].op,
                Op::Delete {
                    calendar: CALENDAR.into(),
                    event: "abc".into(),
                    etag: "\"m1\"".into()
                }
            );
            assert!(matches!(plan.steps[0].conflict, Recheck::Series { .. }));
            assert_eq!(
                plan.removes,
                Removes::Series {
                    series: "abc".into()
                }
            );
        }
    }

    /// T8: "this and following" cuts the series before the occurrence and cancels the original's own
    /// copies of later changed occurrences; already-cancelled ones are left alone.
    #[test]
    fn deleting_this_and_following_cuts_the_series() {
        let renamed = with(plain("2026-11-02T09:00:00Z"), "summary", json!("Off-site"));
        let case = Case::new(weekly(), monday_19()).exceptions(vec![
            renamed,
            cancelled("2026-11-09T09:00:00Z"),
            // Before the split: untouched.
            with(
                plain("2026-10-12T09:00:00+01:00"),
                "summary",
                json!("Early"),
            ),
        ]);
        let plan = case.delete(Scope::Following).unwrap();
        assert_eq!(plan.scope, Scope::Following);
        assert_eq!(plan.steps.len(), 2);
        assert_eq!(
            body(&plan.steps[0]),
            &json!({ "recurrence": ["RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261019T075959Z"] })
                .as_object()
                .unwrap()
                .clone()
        );
        assert_eq!(plan.steps[0].stage, Stage::Commit);
        assert!(matches!(
            &plan.steps[1].op,
            Op::Cancel { event, .. } if event == "abc_20261102T090000Z"
        ));
        assert_eq!(plan.steps[1].stage, Stage::Cleanup);
        assert_eq!(
            plan.removes,
            Removes::From {
                series: "abc".into(),
                at: point("2026-10-19T08:00:00Z"),
                left: Vec::new(),
            }
        );
        // T11: a 412 on the cut where only the description changed goes ahead; a changed rule doesn't.
        let described = with(weekly(), "description", json!("New notes"));
        assert_eq!(plan.resend(0, &described), Resend::Proceed);
        let retimed = with(
            weekly(),
            "recurrence",
            json!(["RRULE:FREQ=WEEKLY;BYDAY=TU"]),
        );
        assert_eq!(
            plan.resend(0, &retimed),
            Resend::Conflict(vec!["recurrence".into()])
        );
        // T11: the cut's reply read by meaning; the old rule kept is "not applied".
        let landed = with(
            weekly(),
            "recurrence",
            json!(["RRULE:UNTIL=20261019T085959;FREQ=WEEKLY;BYDAY=MO"]),
        );
        assert_eq!(plan.verdict(0, Some(&landed)), Applied::Yes);
        assert_eq!(plan.verdict(0, Some(&weekly())), Applied::No);
        let other = with(weekly(), "recurrence", json!(["RRULE:FREQ=DAILY"]));
        assert_eq!(plan.verdict(0, Some(&other)), Applied::Unknown);
        // DS10, C8b review EXEC-7: a cleanup cancel meeting a 412 goes again only while nothing it
        // would throw away changed: a guest's reply moves the etag alone; a new title is a change.
        let replied = with(
            with(plain("2026-11-02T09:00:00Z"), "summary", json!("Off-site")),
            "etag",
            json!("\"i2\""),
        );
        assert_eq!(plan.resend(1, &replied), Resend::Proceed);
        assert_eq!(
            plan.resend(1, &plain("2026-11-02T09:00:00Z")),
            Resend::Conflict(vec!["summary".into()])
        );
        assert_eq!(
            plan.resend(1, &with(replied, "description", json!("Moved to the hall"))),
            Resend::Conflict(vec!["description".into()])
        );
        // Already cancelled, or gone, it's done.
        assert_eq!(
            plan.verdict(1, Some(&cancelled("2026-11-02T09:00:00Z"))),
            Applied::Yes
        );
        assert_eq!(plan.verdict(1, None), Applied::Yes);
    }

    #[test]
    fn a_delete_goes_ahead_only_on_what_the_user_saw() {
        let mut case = Case::new(weekly(), monday_19());
        // Moved in Google since the user saw it.
        case.instance = fresh(moved(
            "2026-10-19T09:00:00+01:00",
            "2026-10-19T15:00:00+01:00",
            "2026-10-19T16:00:00+01:00",
        ));
        for scope in [Scope::This, Scope::Following, Scope::All] {
            assert_eq!(
                plan_series_delete(
                    scope,
                    &case.input(),
                    &seen_of(&case.opened_instance),
                    Notify::None
                ),
                Err(NotPlanned::Conflict(vec!["start".into(), "end".into()]))
            );
        }
        case.instance = fresh(cancelled("2026-10-19T09:00:00+01:00"));
        assert_eq!(
            plan_series_delete(
                Scope::This,
                &case.input(),
                &seen_of(&case.opened_instance),
                Notify::None
            ),
            Err(NotPlanned::Gone)
        );
    }

    /// T8: the series' own facts refuse; a split sibling only warns.
    #[test]
    fn what_refuses_a_series_plan_and_what_only_warns() {
        let theirs = with(
            weekly(),
            "organizer",
            json!({ "email": "boss@example.com" }),
        );
        assert_eq!(
            Case::new(theirs, monday_19()).delete(Scope::All),
            Err(NotPlanned::ReadOnly(ReadOnlyReason::NotOrganizer))
        );
        let focus = with(weekly(), "eventType", json!("focusTime"));
        assert_eq!(
            Case::new(focus, monday_19()).delete(Scope::All),
            Err(NotPlanned::ReadOnly(ReadOnlyReason::SpecialType))
        );
        let elsewhere = with(monday_19(), "recurringEventId", json!("other"));
        assert_eq!(
            Case::new(weekly(), elsewhere).delete(Scope::This),
            Err(NotPlanned::Refused(SeriesRefusal::NotInSeries))
        );
        let mut split = Case::new(weekly(), monday_19());
        split.sibling = true;
        let plan = split.delete(Scope::Following).unwrap();
        assert!(plan.warnings.contains(&SeriesWarning::SplitSibling));
        assert_eq!(plan.steps.len(), 1);
        // Exceptions PM couldn't all see: a cut would leave them behind.
        let mut partial = Case::new(weekly(), monday_19());
        partial.exceptions.complete = false;
        assert_eq!(
            partial.delete(Scope::Following),
            Err(NotPlanned::Refused(SeriesRefusal::Incomplete))
        );
    }

    // --- T9: "this and following" ---

    /// T9: a title change on an endless weekly series: the new series first, then the cut. The new
    /// series is the original's own fields from Google, the new title, the same rule, the original's
    /// zone, and nothing Google mints or PM can't carry.
    #[test]
    fn a_split_inserts_the_rest_first_then_cuts_the_original() {
        let original = with(
            with(
                with(
                    weekly(),
                    "attendees",
                    json!([{ "email": "me@example.com", "self": true, "organizer": true,
                             "responseStatus": "accepted" }]),
                ),
                "conferenceData",
                json!({ "conferenceId": "meet-1" }),
            ),
            "extendedProperties",
            json!({ "shared": { "team": "core" } }),
        );
        let case = Case::new(original, monday_19());
        let plan = case.edit(Scope::Following, title("Retro")).unwrap();
        assert_eq!(plan.scope, Scope::Following);
        let insert = &plan.steps[0];
        assert_eq!(insert.stage, Stage::Commit);
        let note = body(insert)["extendedProperties"]["private"]["pmSplitPlan"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(note.len(), 16);
        assert_eq!(
            insert.shows,
            Shows::Tail {
                of: "abc".into(),
                plan: note.clone()
            }
        );
        assert_eq!(
            insert.undo,
            Some(Undo::DeleteInserted {
                calendar: CALENDAR.into(),
                event: TAIL.into(),
                split_of: "abc".into(),
                plan: note.clone(),
            })
        );
        let tail = body(insert);
        assert_eq!(tail["summary"], "Retro");
        assert_eq!(tail["description"], "Daily notes");
        assert_eq!(
            tail["start"],
            json!({ "dateTime": "2026-10-19T09:00:00+01:00", "timeZone": "Europe/London" })
        );
        assert_eq!(
            tail["end"],
            json!({ "dateTime": "2026-10-19T10:00:00+01:00", "timeZone": "Europe/London" })
        );
        assert_eq!(tail["recurrence"], json!(["RRULE:FREQ=WEEKLY;BYDAY=MO"]));
        assert_eq!(tail["attendees"], json!([{ "email": "me@example.com" }]));
        assert_eq!(
            tail["extendedProperties"],
            json!({
                "shared": { "team": "core" },
                "private": { "pmSplitOf": "abc", "pmSplitPlan": note }
            })
        );
        for never in [
            "id",
            "iCalUID",
            "conferenceData",
            "attachments",
            "eventLabelId",
            "etag",
        ] {
            assert!(!tail.contains_key(never), "{never}");
        }
        // Inserted under PM's id, with conferenceDataVersion=1 so Google adds no call of its own.
        let request = insert.op.request(Notify::None).unwrap();
        assert_eq!(request.body.as_ref().unwrap()["id"], TAIL);
        assert!(request.url.contains("conferenceDataVersion=1"));
        assert!(plan.warnings.contains(&SeriesWarning::NotCarried {
            attachments: false,
            conference: true,
            label: false
        }));
        assert!(plan.warnings.contains(&SeriesWarning::NewIdentity));
        let cut = &plan.steps[1];
        assert_eq!(
            body(cut)["recurrence"],
            json!(["RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261019T075959Z"])
        );
        // AS10: a 412 on the cut where a field the new series copied changed is a conflict.
        assert_eq!(
            plan.resend(
                1,
                &with(case.master.json().clone(), "location", json!("Room B"))
            ),
            Resend::Conflict(vec!["location".into()])
        );
        assert_eq!(
            plan.expect,
            vec![
                Expect::EndsBefore {
                    series: "abc".into(),
                    at: point("2026-10-19T08:00:00Z"),
                    left: Vec::new(),
                },
                Expect::Present {
                    series: TAIL.into()
                }
            ]
        );
        for step in &plan.steps {
            assert!(step.op.fits(), "{:?}", step.op);
        }
        // C8b review EXEC-2: the cut's own verdict, which "Remove the new series" has to see first.
        let landed = with(
            weekly(),
            "recurrence",
            json!(["RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261019T075959Z"]),
        );
        assert_eq!(plan.verdict(1, Some(&landed)), Applied::Yes);
        assert_eq!(plan.verdict(1, Some(&weekly())), Applied::No);
        // A cut answered 404/410 proves it not done: the original isn't there to cut.
        assert_eq!(plan.verdict(1, None), Applied::No);
        // EXEC-9: the insert landed only if what's under the id is this plan's new series of this one;
        // another plan's tail under a reused id is unknown, never this one (fix review).
        let tail_copy = |of: &str, plan: &str| {
            with(
                plain("2026-10-19T09:00:00+01:00"),
                "extendedProperties",
                json!({ "private": { "pmSplitOf": of, "pmSplitPlan": plan } }),
            )
        };
        assert_eq!(
            plan.verdict(0, Some(&tail_copy("abc", &note))),
            Applied::Yes
        );
        assert_eq!(
            plan.verdict(0, Some(&tail_copy("xyz", &note))),
            Applied::Unknown
        );
        assert_eq!(
            plan.verdict(0, Some(&tail_copy("abc", "another plan"))),
            Applied::Unknown
        );
        assert_eq!(plan.verdict(0, Some(&weekly())), Applied::Unknown);
        assert_eq!(plan.verdict(0, None), Applied::Unknown);
        // The same plan made again carries the same note; another draft another.
        let again = case.edit(Scope::Following, title("Retro")).unwrap();
        assert_eq!(again.steps[0].shows, insert.shows);
        let other = case.edit(Scope::Following, title("Retro 2")).unwrap();
        assert_ne!(other.steps[0].shows, insert.shows);
        // EXEC-11: its undo is done on 404/410, and sent again on a 412 while the series is PM's.
        let undo = plan.steps[0].undo.as_ref().unwrap();
        assert!(undo.gone_is_done());
        assert!(undo.resend_on(&tail_copy("abc", &note)));
        assert!(!undo.resend_on(&weekly()));
        assert!(!undo.resend_on(&cancelled("2026-10-19T09:00:00+01:00")));
    }

    /// T9: a COUNT of 10 split at its fifth occurrence leaves 6 for the rest (k = 4, from Google).
    #[test]
    fn a_split_rebases_a_count_on_googles_number() {
        let counted = master(&["RRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=10"]);
        let fifth = plain("2026-11-02T09:00:00Z");
        let mut case = Case::new(counted, fifth.clone());
        // Without Google's count there's no rebasing a COUNT.
        assert_eq!(
            case.edit(Scope::Following, title("Retro")),
            Err(NotPlanned::Refused(SeriesRefusal::Recur(
                RecurError::CannotCount
            )))
        );
        case.instances = Some(Listing {
            items: vec![
                plain("2026-10-05T09:00:00+01:00"),
                plain("2026-10-12T09:00:00+01:00"),
                plain("2026-10-19T09:00:00+01:00"),
                plain("2026-10-26T09:00:00Z"),
                fifth,
            ],
            complete: true,
        });
        let plan = case.edit(Scope::Following, title("Retro")).unwrap();
        assert_eq!(
            body(&plan.steps[0])["recurrence"],
            json!(["RRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=6"])
        );
        assert_eq!(
            body(&plan.steps[1])["recurrence"],
            json!(["RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261102T085959Z"])
        );
    }

    /// T9, A6: the rest moved an hour later takes its EXDATEs and its cancelled occurrences with it,
    /// and each later occurrence changed on its own is carried onto the same occurrence of the rest,
    /// then cancelled on the original only once the carry is done (Q3, DS3).
    #[test]
    fn a_split_with_a_new_time_carries_everything_after_it() {
        let with_exdate = master(&[
            "RRULE:FREQ=WEEKLY;BYDAY=MO",
            "EXDATE;TZID=Europe/London:20261102T090000",
        ]);
        let renamed = with(plain("2026-11-09T09:00:00Z"), "summary", json!("Off-site"));
        let moved_one = moved(
            "2026-11-16T09:00:00Z",
            "2026-11-17T14:00:00Z",
            "2026-11-17T15:00:00Z",
        );
        let case = Case::new(with_exdate, monday_19()).exceptions(vec![
            cancelled("2026-10-26T09:00:00Z"),
            renamed,
            moved_one,
        ]);
        let plan = case
            .edit(Scope::Following, at("10:00", "2026-10-19"))
            .unwrap();
        let tail = body(&plan.steps[0]);
        assert_eq!(
            tail["start"],
            json!({ "dateTime": "2026-10-19T10:00:00+01:00", "timeZone": "Europe/London" })
        );
        assert_eq!(
            tail["recurrence"],
            json!([
                "RRULE:FREQ=WEEKLY;BYDAY=MO",
                "EXDATE;TZID=Europe/London:20261102T100000",
                "EXDATE:20261026T100000Z"
            ])
        );
        // The renamed 09-11: its title onto the rest's 09-11 at 10:00; then its old copy cancelled.
        assert_eq!(
            plan.steps[2].op,
            Op::Carry {
                calendar: CALENDAR.into(),
                series: TAIL.into(),
                original: point("2026-11-09T10:00:00Z"),
                body: json!({ "summary": "Off-site" })
                    .as_object()
                    .unwrap()
                    .clone()
            }
        );
        assert_eq!(plan.steps[2].stage, Stage::Cleanup);
        assert!(
            matches!(&plan.steps[3].op, Op::Cancel { event, .. } if event == "abc_20261109T090000Z")
        );
        assert_eq!(plan.steps[3].requires, Some(2));
        // The one moved on its own keeps its own time.
        assert_eq!(
            body(&plan.steps[4]),
            &json!({
                "start": { "dateTime": "2026-11-17T14:00:00Z", "timeZone": "Europe/London" },
                "end": { "dateTime": "2026-11-17T15:00:00Z", "timeZone": "Europe/London" },
            })
            .as_object()
            .unwrap()
            .clone()
        );
        assert_eq!(plan.steps[5].requires, Some(4));
        assert_eq!(plan.steps.len(), 6);
        // A carry is sent once its occurrence on the rest is found; never to an id that isn't one.
        let carry = plan.steps[2]
            .op
            .carry_request("tail_x", "\"e\"", Notify::None)
            .unwrap();
        assert_eq!(carry.body, Some(json!({ "summary": "Off-site" })));
        assert_eq!(carry.if_match.as_deref(), Some("\"e\""));
        assert!(plan.steps[2].op.request(Notify::None).is_none());
        assert!(plan.steps[2]
            .op
            .carry_request("..", "\"e\"", Notify::None)
            .is_none());
        // A3: every step, and every undo, sent with the one choice.
        let notify = Notify::All;
        let plan = plan_series_edit(
            Scope::Following,
            &case.input(),
            &at("10:00", "2026-10-19"),
            TAIL,
            notify,
        )
        .unwrap();
        for step in &plan.steps {
            assert_eq!(step.notify, notify);
            if let Some(r) = step.op.request(step.notify) {
                assert!(r.url.contains("sendUpdates=all"), "{}", r.url);
            }
        }
        let undo = plan.steps[0]
            .undo
            .as_ref()
            .unwrap()
            .request("\"t1\"", notify);
        assert!(undo.url.contains("sendUpdates=all"));
        assert_eq!(undo.if_match.as_deref(), Some("\"t1\""));
    }

    /// The occurrence the split starts at, changed on its own: its own changes go with it, except
    /// what the draft sets.
    #[test]
    fn the_split_occurrence_keeps_its_own_changes_under_the_drafts() {
        let own = with(
            with(monday_19(), "summary", json!("Standup (room 4)")),
            "location",
            json!("Room 4"),
        );
        let case = Case::new(weekly(), own.clone()).exceptions(vec![own]);
        let plan = case.edit(Scope::Following, title("Retro")).unwrap();
        assert_eq!(body(&plan.steps[0])["summary"], "Retro");
        assert_eq!(
            plan.steps[2].op,
            Op::Carry {
                calendar: CALENDAR.into(),
                series: TAIL.into(),
                original: point("2026-10-19T08:00:00Z"),
                body: json!({ "location": "Room 4" }).as_object().unwrap().clone()
            }
        );
    }

    #[test]
    fn a_split_refuses_what_it_cant_carry() {
        let attached = with(
            plain("2026-11-02T09:00:00Z"),
            "attachments",
            json!([{ "fileUrl": "https://drive.google.com/agenda" }]),
        );
        let case = Case::new(weekly(), monday_19()).exceptions(vec![attached]);
        assert_eq!(
            case.edit(Scope::Following, title("Retro")),
            Err(NotPlanned::Refused(SeriesRefusal::CannotCarry(
                NaiveDate::from_ymd_opt(2026, 11, 2).unwrap()
            )))
        );
        // Q4 via retime: a weekly move onto another repeat day is refused for the rest too.
        let two_days = master(&["RRULE:FREQ=WEEKLY;BYDAY=MO,WE"]);
        let case = Case::new(two_days, monday_19());
        assert!(matches!(
            case.edit(Scope::Following, at("09:00", "2026-10-21")),
            Err(NotPlanned::Refused(SeriesRefusal::Retime(_)))
        ));
    }

    // --- "all events" and "this event" ---

    /// T9: "this and following" at the series' first occurrence is the "all events" plan.
    #[test]
    fn a_split_at_the_first_occurrence_is_all_events() {
        let case = Case::new(weekly(), plain("2026-10-05T09:00:00+01:00"));
        let following = case.edit(Scope::Following, title("Retro")).unwrap();
        let all = case.edit(Scope::All, title("Retro")).unwrap();
        assert_eq!(following, all);
        assert_eq!(all.scope, Scope::All);
        assert_eq!(
            all.steps[0].op,
            Op::Patch {
                calendar: CALENDAR.into(),
                event: "abc".into(),
                etag: "\"m1\"".into(),
                body: json!({ "summary": "Retro" }).as_object().unwrap().clone()
            }
        );
    }

    /// T7 through the planner: "all events" an hour later moves the series' start and its EXDATEs,
    /// and adds an EXDATE for each cancelled occurrence where it now falls; its undo puts the old
    /// start, end and rule back.
    #[test]
    fn all_events_at_a_new_time_moves_the_series() {
        let with_exdate = master(&[
            "RRULE:FREQ=WEEKLY;BYDAY=MO",
            "EXDATE;TZID=Europe/London:20261102T090000",
        ]);
        let renamed = with(plain("2026-11-09T09:00:00Z"), "summary", json!("Off-site"));
        let case = Case::new(with_exdate, monday_19())
            .exceptions(vec![cancelled("2026-10-26T09:00:00Z"), renamed]);
        let plan = case.edit(Scope::All, at("10:00", "2026-10-19")).unwrap();
        let sent = body(&plan.steps[0]);
        assert_eq!(
            sent["start"],
            json!({ "dateTime": "2026-10-05T10:00:00+01:00", "timeZone": "Europe/London" })
        );
        assert_eq!(
            sent["recurrence"],
            json!([
                "RRULE:FREQ=WEEKLY;BYDAY=MO",
                "EXDATE;TZID=Europe/London:20261102T100000",
                "EXDATE:20261026T100000Z"
            ])
        );
        assert!(matches!(
            &plan.steps[0].undo,
            Some(Undo::RestoreMaster { body, .. })
                if body["start"]["dateTime"] == "2026-10-05T09:00:00+01:00"
        ));
        // The renamed occurrence's time is the series': Google may reset it.
        assert!(plan
            .warnings
            .contains(&SeriesWarning::ExceptionsMayReset(1)));
        // Moving the day too moves the series' start, and says so.
        let tuesdays = case.edit(Scope::All, at("09:00", "2026-10-20")).unwrap();
        assert!(tuesdays.warnings.contains(&SeriesWarning::StartMoved {
            from: NaiveDate::from_ymd_opt(2026, 10, 5).unwrap(),
            to: NaiveDate::from_ymd_opt(2026, 10, 6).unwrap()
        }));
        assert!(tuesdays
            .warnings
            .contains(&SeriesWarning::RepeatDayChanges {
                from: NaiveDate::from_ymd_opt(2026, 10, 19).unwrap(),
                to: NaiveDate::from_ymd_opt(2026, 10, 20).unwrap()
            }));
        assert!(!plan
            .warnings
            .iter()
            .any(|w| matches!(w, SeriesWarning::RepeatDayChanges { .. })));
    }

    /// A one-day weekly series moved a whole week, now allowed (16-10): it keeps its weekday, and
    /// every occurrence still moves a week, so both warnings are raised, for C9 to word as "every
    /// occurrence a week later" rather than "every Monday → every Monday". The rest of a split, which
    /// has no start-moved warning, is told the same.
    #[test]
    fn a_whole_week_on_says_every_occurrence_moves() {
        let case = Case::new(weekly(), monday_19());
        let a_week = SeriesWarning::RepeatDayChanges {
            from: NaiveDate::from_ymd_opt(2026, 10, 19).unwrap(),
            to: NaiveDate::from_ymd_opt(2026, 10, 26).unwrap(),
        };
        let all = case.edit(Scope::All, at("09:00", "2026-10-26")).unwrap();
        assert!(all.warnings.contains(&SeriesWarning::StartMoved {
            from: NaiveDate::from_ymd_opt(2026, 10, 5).unwrap(),
            to: NaiveDate::from_ymd_opt(2026, 10, 12).unwrap()
        }));
        assert!(all.warnings.contains(&a_week));
        let following = case
            .edit(Scope::Following, at("09:00", "2026-10-26"))
            .unwrap();
        assert!(following.warnings.contains(&a_week));
    }

    #[test]
    fn all_events_checks_against_the_series_as_it_was_opened() {
        let mut case = Case::new(weekly(), monday_19());
        // Renamed in Google since the editor opened.
        case.master = fresh(with(weekly(), "summary", json!("Standup v2")));
        assert_eq!(
            case.edit(Scope::All, title("Retro")),
            Err(NotPlanned::Conflict(vec!["summary".into()]))
        );
        // No series as it was opened: no "all events".
        let input = SeriesInput {
            opened: Opened {
                instance: &case.opened_instance,
                master: None,
            },
            ..case.input()
        };
        assert_eq!(
            plan_series_edit(Scope::All, &input, &title("Retro"), TAIL, Notify::None),
            Err(NotPlanned::Refused(SeriesRefusal::NoBase))
        );
    }

    /// AS17: setting a changed occurrence back to the series' value with "all events" changes nothing
    /// in the series; it says to choose "this event".
    #[test]
    fn all_events_with_the_series_own_value_points_to_this_event() {
        let own = with(monday_19(), "summary", json!("Standup (room 4)"));
        let case = Case::new(weekly(), own);
        assert_eq!(
            case.edit(Scope::All, title("Standup")),
            Err(NotPlanned::Refused(SeriesRefusal::OnlyThisDiffers))
        );
        assert_eq!(
            Case::new(weekly(), monday_19()).edit(Scope::All, title("Standup")),
            Err(NotPlanned::NoChange)
        );
    }

    #[test]
    fn this_event_patches_the_occurrence_only() {
        let case = Case::new(weekly(), monday_19());
        let plan = case
            .edit(
                Scope::This,
                EventPatchDraft {
                    summary: Some("Retro".into()),
                    show_as: Some(ShowAs::Free),
                    ..Default::default()
                },
            )
            .unwrap();
        assert_eq!(plan.steps.len(), 1);
        assert!(matches!(
            &plan.steps[0].op,
            Op::Patch { event, .. } if event == "abc_20261019T080000Z"
        ));
        assert_eq!(
            body(&plan.steps[0]),
            &json!({ "summary": "Retro", "transparency": "transparent" })
                .as_object()
                .unwrap()
                .clone()
        );
        assert_eq!(
            case.edit(
                Scope::This,
                EventPatchDraft {
                    visibility: Some(crate::calendar_write::dto::Visibility::Private),
                    ..Default::default()
                }
            ),
            Err(NotPlanned::Refused(SeriesRefusal::VisibilityIsSeriesWide))
        );
    }

    /// DS9: a lost answer is read by meaning: start and end as instants, a rule by what it says.
    #[test]
    fn a_lost_answer_is_read_by_meaning() {
        let case = Case::new(weekly(), monday_19());
        let plan = case.edit(Scope::This, at("10:00", "2026-10-19")).unwrap();
        // Google returns the new time in another zone's offset.
        let landed = moved(
            "2026-10-19T09:00:00+01:00",
            "2026-10-19T09:00:00+00:00",
            "2026-10-19T10:00:00+00:00",
        );
        assert_eq!(plan.verdict(0, Some(&landed)), Applied::Yes);
        assert_eq!(plan.verdict(0, Some(case.instance.json())), Applied::No);
    }

    // --- C8b review ---

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    /// The weekly series with two guests besides the user.
    fn with_guests() -> Value {
        with(
            weekly(),
            "attendees",
            json!([
                { "email": "me@example.com", "self": true, "organizer": true, "responseStatus": "accepted" },
                { "email": "a@example.com", "responseStatus": "needsAction" },
                { "email": "b@example.com", "responseStatus": "needsAction" },
            ]),
        )
    }

    /// G5: a series with guests stays read-only until C12, whatever the occurrence shows. Once guests
    /// are allowed, an occurrence whose guests were removed on its own is a change PM can't carry.
    #[test]
    fn a_series_with_guests_is_judged_by_its_own_guest_list() {
        let no_guests_here = monday_19();
        let case = Case::new(with_guests(), no_guests_here);
        for scope in [Scope::All, Scope::Following] {
            assert_eq!(
                case.edit(scope, title("Retro")),
                Err(NotPlanned::ReadOnly(ReadOnlyReason::HasGuests))
            );
        }
        assert_eq!(
            case.delete(Scope::Following),
            Err(NotPlanned::ReadOnly(ReadOnlyReason::HasGuests))
        );
        // Only the user on the list isn't a guest.
        let just_me = with(
            weekly(),
            "attendees",
            json!([{ "email": "me@example.com", "self": true }]),
        );
        assert!(Case::new(just_me, monday_19())
            .edit(Scope::All, title("Retro"))
            .is_ok());
        let solo = with(
            plain("2026-10-26T09:00:00Z"),
            "summary",
            json!("Solo review"),
        );
        let case = Case::new(with_guests(), monday_19())
            .exceptions(vec![solo])
            .guests();
        assert_eq!(
            case.edit(Scope::Following, title("Retro")),
            Err(NotPlanned::Refused(SeriesRefusal::CannotCarry(day(
                2026, 10, 26
            ))))
        );
    }

    /// EXEC-1, EXEC-6, G1: guests are compared by who they are. A reply between opening and saving,
    /// or between the insert and the cut, changes nothing the new series copied; a guest added, or a
    /// shared property changed, does.
    #[test]
    fn a_split_watches_the_guests_it_copies_but_not_their_replies() {
        let mut case = Case::new(with_guests(), monday_19()).guests();
        let replied = with(
            weekly(),
            "attendees",
            json!([
                { "email": "me@example.com", "self": true, "responseStatus": "accepted" },
                { "email": "a@example.com", "responseStatus": "accepted" },
                { "email": "b@example.com", "responseStatus": "declined" },
            ]),
        );
        case.master = fresh(replied.clone());
        let plan = case.edit(Scope::Following, title("Retro")).unwrap();
        assert_eq!(
            body(&plan.steps[0])["attendees"],
            json!([
                { "email": "me@example.com" },
                { "email": "a@example.com" },
                { "email": "b@example.com" },
            ])
        );
        let added = with(
            replied.clone(),
            "attendees",
            json!([
                { "email": "a@example.com" },
                { "email": "b@example.com" },
                { "email": "c@example.com" },
            ]),
        );
        assert_eq!(plan.resend(1, &replied), Resend::Proceed);
        assert_eq!(
            plan.resend(1, &added),
            Resend::Conflict(vec!["attendees".into()])
        );
        assert_eq!(
            plan.resend(
                1,
                &with(
                    replied,
                    "extendedProperties",
                    json!({ "shared": { "team": "infra" } })
                )
            ),
            Resend::Conflict(vec!["extendedProperties".into()])
        );
        case.master = fresh(added);
        assert_eq!(
            case.edit(Scope::Following, title("Retro")),
            Err(NotPlanned::Conflict(vec!["attendees".into()]))
        );
    }

    /// R1, EXEC-5: a split cancels every stored exception from it on, as a delete does: one with
    /// nothing to carry is cancelled straight away, at the split or after it.
    #[test]
    fn a_split_cancels_stored_exceptions_with_nothing_to_carry() {
        let reread = |start: &str| with(plain(start), "sequence", json!(1));
        let case = Case::new(weekly(), reread("2026-10-19T09:00:00+01:00")).exceptions(vec![
            reread("2026-10-19T09:00:00+01:00"),
            reread("2026-11-02T09:00:00Z"),
        ]);
        let plan = case.edit(Scope::Following, title("Retro")).unwrap();
        assert_eq!(plan.steps.len(), 4);
        for (step, event) in plan.steps[2..]
            .iter()
            .zip(["abc_20261019T080000Z", "abc_20261102T090000Z"])
        {
            assert!(matches!(&step.op, Op::Cancel { event: e, .. } if e == event));
            assert_eq!(step.requires, None);
            assert_eq!(step.stage, Stage::Cleanup);
        }
    }

    /// EXEC-8, G8: an occurrence with its own visibility can't pass it to one occurrence of the new
    /// series (Google spreads a stricter one to the whole series and ignores a looser one).
    #[test]
    fn a_split_refuses_an_occurrence_with_its_own_visibility() {
        let private = with(
            plain("2026-11-02T09:00:00Z"),
            "visibility",
            json!("private"),
        );
        assert_eq!(
            Case::new(weekly(), monday_19())
                .exceptions(vec![private])
                .edit(Scope::Following, title("Retro")),
            Err(NotPlanned::Refused(SeriesRefusal::CannotCarry(day(
                2026, 11, 2
            ))))
        );
    }

    /// G4: one occurrence of an all-day series made timed is carried with the all-day key cleared,
    /// since Google merges the node; the carry then reads as landed.
    #[test]
    fn a_carry_between_timed_and_all_day_clears_the_other_kind() {
        let all_day = |original: &str, start: Value, end: Value| {
            json!({
                "kind": "calendar#event",
                "id": format!("abc_{}", original.replace('-', "")),
                "etag": "\"d1\"",
                "status": "confirmed",
                "summary": "Standup",
                "description": "Daily notes",
                "recurringEventId": "abc",
                "originalStartTime": { "date": original },
                "start": start,
                "end": end,
                "organizer": { "email": "me@example.com", "self": true },
                "reminders": { "useDefault": true },
            })
        };
        let series = with(
            with(weekly(), "start", json!({ "date": "2026-10-05" })),
            "end",
            json!({ "date": "2026-10-06" }),
        );
        let s = all_day(
            "2026-10-19",
            json!({ "date": "2026-10-19" }),
            json!({ "date": "2026-10-20" }),
        );
        let timed = all_day(
            "2026-11-02",
            json!({ "dateTime": "2026-11-02T10:00:00Z", "timeZone": "Europe/London" }),
            json!({ "dateTime": "2026-11-02T11:00:00Z", "timeZone": "Europe/London" }),
        );
        let plan = Case::new(series, s)
            .exceptions(vec![timed])
            .edit(Scope::Following, title("Retro"))
            .unwrap();
        let carried = body(&plan.steps[2]);
        assert_eq!(
            carried["start"],
            json!({ "dateTime": "2026-11-02T10:00:00Z", "timeZone": "Europe/London", "date": null })
        );
        let landed = json!({
            "start": { "dateTime": "2026-11-02T10:00:00Z", "timeZone": "Europe/London" },
            "end": { "dateTime": "2026-11-02T11:00:00Z", "timeZone": "Europe/London" },
        });
        assert_eq!(plan.verdict(2, Some(&landed)), Applied::Yes);
        assert_eq!(plan.verdict(2, Some(&json!({}))), Applied::Unknown);
        assert_eq!(plan.verdict(2, None), Applied::No);
    }

    /// G6: a listing by iCal UID holds the master too; it's no exception of its own.
    #[test]
    fn the_master_in_its_own_listing_is_no_exception() {
        let plan = Case::new(weekly(), monday_19())
            .exceptions(vec![weekly()])
            .edit(Scope::All, at("10:00", "2026-10-19"))
            .unwrap();
        assert!(!plan
            .warnings
            .iter()
            .any(|w| matches!(w, SeriesWarning::ExceptionsMayReset(_))));
    }

    /// G7: an end moved into another zone is a zone change for the series rows, as a start is.
    #[test]
    fn an_end_in_another_zone_is_a_zone_change() {
        let draft = EventPatchDraft {
            time: Some(TimeDraft::Timed {
                start_date: "2026-10-19".into(),
                start_time: "10:00".into(),
                start_zone: "Europe/London".into(),
                end_date: "2026-10-19".into(),
                end_time: "12:00".into(),
                end_zone: "Europe/Paris".into(),
            }),
            ..Default::default()
        };
        let case = Case::new(weekly(), monday_19());
        for scope in [Scope::All, Scope::Following] {
            assert_eq!(
                case.edit(scope, draft.clone()),
                Err(NotPlanned::Refused(SeriesRefusal::ZoneChange))
            );
        }
    }

    /// ADV-1: a new time is read against the occurrence as Google holds it; one moved in Google since
    /// the editor opened is a conflict, never overwritten.
    #[test]
    fn a_new_time_needs_the_occurrence_where_the_user_saw_it() {
        let mut case = Case::new(weekly(), monday_19());
        let moved_since = moved(
            "2026-10-19T09:00:00+01:00",
            "2026-10-19T14:00:00+01:00",
            "2026-10-19T15:00:00+01:00",
        );
        case.instance = fresh(moved_since.clone());
        case.exceptions = complete(vec![moved_since]);
        for scope in [Scope::All, Scope::Following] {
            assert_eq!(
                case.edit(scope, at("10:00", "2026-10-19")),
                Err(NotPlanned::Conflict(vec!["start".into(), "end".into()]))
            );
        }
    }

    /// ADV-2, ADV-3: putting a moved occurrence back on its series' slot, or giving the rest what it
    /// already has, changes nothing in the series: no PATCH, no new series.
    #[test]
    fn a_change_only_this_occurrence_needs_moves_nothing_else() {
        let moved_s = moved(
            "2026-10-19T09:00:00+01:00",
            "2026-10-19T11:00:00+01:00",
            "2026-10-19T12:00:00+01:00",
        );
        let case = Case::new(weekly(), moved_s.clone())
            .exceptions(vec![cancelled("2026-10-26T09:00:00Z"), moved_s]);
        for scope in [Scope::All, Scope::Following] {
            assert_eq!(
                case.edit(scope, at("09:00", "2026-10-19")),
                Err(NotPlanned::Refused(SeriesRefusal::OnlyThisDiffers))
            );
        }
        let own = with(monday_19(), "summary", json!("Standup (room 4)"));
        let case = Case::new(weekly(), own.clone()).exceptions(vec![own]);
        assert_eq!(
            case.edit(Scope::Following, title("Standup")),
            Err(NotPlanned::Refused(SeriesRefusal::OnlyThisDiffers))
        );
        assert_eq!(
            Case::new(weekly(), monday_19()).edit(Scope::Following, title("Standup")),
            Err(NotPlanned::NoChange)
        );
    }

    /// ADV-4: a series starting off its own rule (RFC 5545 counts that start, the walk doesn't)
    /// can't have its COUNT rebased.
    #[test]
    fn a_count_on_a_series_starting_off_its_rule_is_refused() {
        let sunday = master_with(
            &["RRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=5"],
            "2026-10-04T09:00:00+01:00",
            "2026-10-04T10:00:00+01:00",
        );
        let mut case = Case::new(sunday, plain("2026-10-12T09:00:00+01:00"));
        case.instances = Some(Listing {
            items: vec![
                plain("2026-10-04T09:00:00+01:00"),
                plain("2026-10-05T09:00:00+01:00"),
                plain("2026-10-12T09:00:00+01:00"),
            ],
            complete: true,
        });
        assert_eq!(
            case.edit(Scope::Following, title("Retro")),
            Err(NotPlanned::Refused(SeriesRefusal::Recur(
                RecurError::CannotCount
            )))
        );
    }

    /// ADV-5: the new series' id is never the original's.
    #[test]
    fn a_split_refuses_the_series_own_id_for_the_rest() {
        let case = Case::new(weekly(), monday_19());
        assert_eq!(
            plan_series_edit(
                Scope::Following,
                &case.input(),
                &title("Retro"),
                "abc",
                Notify::None
            ),
            Err(NotPlanned::Refused(SeriesRefusal::Unreadable))
        );
    }

    /// ADV-8: retiming an occurrence already moved to Wednesday moves the series' day too, and says
    /// so.
    #[test]
    fn a_moved_occurrence_retimed_warns_the_repeat_day_moves() {
        let on_wednesday = moved(
            "2026-10-19T09:00:00+01:00",
            "2026-10-21T09:00:00+01:00",
            "2026-10-21T10:00:00+01:00",
        );
        let case = Case::new(weekly(), on_wednesday.clone()).exceptions(vec![on_wednesday]);
        let warned = SeriesWarning::RepeatDayChanges {
            from: day(2026, 10, 19),
            to: day(2026, 10, 21),
        };
        for scope in [Scope::All, Scope::Following] {
            let plan = case.edit(scope, at("10:00", "2026-10-21")).unwrap();
            assert!(plan.warnings.contains(&warned), "{scope:?}");
        }
    }

    /// ADV-9: the rest of a daily series moved a day earlier would start on a day the part before
    /// it still has.
    #[test]
    fn the_rest_cant_start_where_the_part_before_it_reaches() {
        let daily = master_with(
            &["RRULE:FREQ=DAILY"],
            "2026-10-12T09:00:00+01:00",
            "2026-10-12T10:00:00+01:00",
        );
        let case = Case::new(daily, monday_19());
        assert_eq!(
            case.edit(Scope::Following, at("09:00", "2026-10-18")),
            Err(NotPlanned::Refused(SeriesRefusal::Retime(
                RetimeError::Overlaps(day(2026, 10, 18))
            )))
        );
        assert!(case
            .edit(Scope::Following, at("09:00", "2026-10-20"))
            .is_ok());
    }

    /// EXEC-3: an "all events" move that half landed (Google kept the old rule with the new start,
    /// or the reverse) is the one case its restore is for.
    #[test]
    fn a_half_landed_move_is_told_from_a_whole_one() {
        let case = Case::new(weekly(), monday_19());
        let plan = case.edit(Scope::All, at("09:00", "2026-10-20")).unwrap();
        let start = json!({ "dateTime": "2026-10-06T09:00:00+01:00", "timeZone": "Europe/London" });
        let end = json!({ "dateTime": "2026-10-06T10:00:00+01:00", "timeZone": "Europe/London" });
        let tuesdays = json!(["RRULE:FREQ=WEEKLY;BYDAY=TU"]);
        let new_start = with(with(weekly(), "start", start.clone()), "end", end.clone());
        assert!(plan.half_applied(0, &new_start));
        assert!(plan.half_applied(0, &with(weekly(), "recurrence", tuesdays.clone())));
        let whole = with(new_start.clone(), "recurrence", tuesdays);
        assert!(!plan.half_applied(0, &whole));
        assert!(!plan.half_applied(0, &with(whole, "summary", json!("Retro v2"))));
        assert!(!plan.half_applied(0, &weekly()));
        // A text-only plan has nothing to restore.
        let text = case.edit(Scope::All, title("Retro")).unwrap();
        assert!(!text.half_applied(0, &new_start));
    }

    /// EXEC-12: the whole series' delete re-sent on a 412 only while its title, start and rule are
    /// as planned; gone means done.
    #[test]
    fn a_series_delete_is_read_and_resent_on_the_series() {
        let plan = Case::new(weekly(), monday_19()).delete(Scope::All).unwrap();
        assert_eq!(
            plan.resend(0, &with(weekly(), "location", json!("Hall"))),
            Resend::Proceed
        );
        assert_eq!(
            plan.resend(
                0,
                &with(
                    weekly(),
                    "start",
                    json!({ "dateTime": "2026-10-05T10:00:00+01:00", "timeZone": "Europe/London" })
                )
            ),
            Resend::Conflict(vec!["start".into()])
        );
        assert_eq!(plan.verdict(0, None), Applied::Yes);
        assert_eq!(plan.verdict(0, Some(&weekly())), Applied::No);
    }

    /// R5, R2: a split's mirror removals wait for its new series to show; once the run is over, an
    /// occurrence it couldn't cancel stays in what the re-fetch must show and in the mirror.
    #[test]
    fn a_splits_removals_wait_and_leave_what_stayed_behind() {
        let renamed = with(plain("2026-11-09T09:00:00Z"), "summary", json!("Off-site"));
        let plan = Case::new(weekly(), monday_19())
            .exceptions(vec![renamed])
            .edit(Scope::Following, title("Retro"))
            .unwrap();
        let diff = |stale| SeriesDiff {
            upserts: Vec::new(),
            created: Vec::new(),
            deletes: Vec::new(),
            stale,
        };
        assert_eq!(plan.removes_for(&diff(true), &[]), Removes::Nothing);
        assert_eq!(plan.removes_for(&diff(false), &[]), plan.removes);
        // The cancel of 09-11 skipped: its occurrence stays in what the re-fetch must show, and its
        // row stays; still nothing removed while the new series doesn't show (fix review).
        let left = point("2026-11-09T09:00:00Z");
        let expect = plan.expect_after(&[3]);
        assert!(matches!(&expect[0], Expect::EndsBefore { left: l, .. } if *l == vec![left]));
        assert!(matches!(
            plan.removes_for(&diff(false), &[3]),
            Removes::From { left: l, .. } if l == vec![left]
        ));
        assert_eq!(plan.removes_for(&diff(true), &[3]), Removes::Nothing);
        // A step that isn't a cleanup cancel leaves nothing behind.
        assert!(matches!(
            plan.removes_for(&diff(false), &[1]),
            Removes::From { left: l, .. } if l.is_empty()
        ));
        // A delete removes whatever a re-fetch shows.
        let delete = Case::new(weekly(), monday_19())
            .delete(Scope::Following)
            .unwrap();
        assert_eq!(delete.removes_for(&diff(true), &[]), delete.removes);
    }

    // --- C8b fix review ---

    /// A timed draft on 19-10 from `start` to `end` (HH:MM), both in `zone`.
    fn timed_in(date: &str, start: &str, end: &str, zone: &str) -> EventPatchDraft {
        EventPatchDraft {
            time: Some(TimeDraft::Timed {
                start_date: date.into(),
                start_time: start.into(),
                start_zone: zone.into(),
                end_date: date.into(),
                end_time: end.into(),
                end_zone: zone.into(),
            }),
            ..Default::default()
        }
    }

    /// `event` with its start, end and original start in `zone` (the fixtures are in London).
    fn in_zone(mut event: Value, zone: &str) -> Value {
        for key in ["start", "end", "originalStartTime"] {
            if let Some(node) = event.get_mut(key).and_then(Value::as_object_mut) {
                node.insert("timeZone".into(), json!(zone));
            }
        }
        event
    }

    /// On 28-03-2027 London skips 01:00-02:00, so a 01:30 series' occurrence that day is at 02:30. The
    /// rest of the series can't start there: it would repeat at 02:30, and the EXDATE after it would
    /// name nothing.
    #[test]
    fn the_rest_cant_start_on_a_day_the_clocks_skip_its_time() {
        let small_hours = master_with(
            &[
                "RRULE:FREQ=DAILY",
                "EXDATE;TZID=Europe/London:20270405T013000",
            ],
            "2027-03-20T01:30:00+00:00",
            "2027-03-20T02:30:00+00:00",
        );
        let case = Case::new(small_hours, plain("2027-03-28T01:30:00Z"));
        let refused = Err(NotPlanned::Refused(SeriesRefusal::SplitInGap(day(
            2027, 3, 28,
        ))));
        assert_eq!(case.edit(Scope::Following, title("Retro")), refused);
        assert_eq!(
            case.edit(Scope::Following, at("03:00", "2027-03-28")),
            refused
        );
        // The day after is an ordinary split at the series' own 01:30.
        let next = Case::new(case.master.json().clone(), plain("2027-03-29T00:30:00Z"));
        let plan = next.edit(Scope::Following, title("Retro")).unwrap();
        assert_eq!(
            body(&plan.steps[0])["start"],
            json!({ "dateTime": "2027-03-29T01:30:00+01:00", "timeZone": "Europe/London" })
        );
    }

    /// The part a split keeps ends at its last occurrence still showing: one deleted before the split
    /// doesn't count, and a start off its own rule does (FIX-1, FIX-4).
    #[test]
    fn the_rest_is_measured_against_what_the_part_before_it_still_shows() {
        let daily = |extra: &[&str]| {
            let mut lines = vec!["RRULE:FREQ=DAILY"];
            lines.extend_from_slice(extra);
            master_with(
                &lines,
                "2026-10-12T09:00:00+01:00",
                "2026-10-12T10:00:00+01:00",
            )
        };
        let to_18 = at("09:00", "2026-10-18");
        // 18-10 deleted by an EXDATE, or cancelled on its own: the rest may start there.
        assert!(Case::new(
            daily(&["EXDATE;TZID=Europe/London:20261018T090000"]),
            monday_19()
        )
        .edit(Scope::Following, to_18.clone())
        .is_ok());
        assert!(Case::new(daily(&[]), monday_19())
            .exceptions(vec![cancelled("2026-10-18T09:00:00+01:00")])
            .edit(Scope::Following, to_18.clone())
            .is_ok());
        // An extra date earlier the same day is an occurrence of its own.
        let weekly_extra = master(&[
            "RRULE:FREQ=WEEKLY;BYDAY=MO",
            "RDATE;TZID=Europe/London:20261019T070000",
        ]);
        assert!(Case::new(weekly_extra, monday_19())
            .edit(Scope::Following, at("10:00", "2026-10-19"))
            .is_ok());
        // A start off its own rule (Saturday 10-10 on "the 15th") is kept by the part before.
        let fifteenth = master_with(
            &["RRULE:FREQ=MONTHLY;BYMONTHDAY=15"],
            "2026-10-10T09:00:00+01:00",
            "2026-10-10T10:00:00+01:00",
        );
        let case = Case::new(fifteenth, plain("2026-10-15T09:00:00+01:00"));
        assert_eq!(
            case.edit(Scope::Following, at("09:00", "2026-10-10")),
            Err(NotPlanned::Refused(SeriesRefusal::Retime(
                RetimeError::Overlaps(day(2026, 10, 10))
            )))
        );
        assert!(case
            .edit(Scope::Following, at("09:00", "2026-10-16"))
            .is_ok());
    }

    /// FIX-2: "all events" with a new title and a moved occurrence put back on its slot would save the
    /// title and drop the time; so would a new length, which an occurrence with its own time doesn't
    /// take. Each says only this occurrence's time differs.
    #[test]
    fn all_events_never_saves_half_a_draft() {
        let moved_s = moved(
            "2026-10-19T09:00:00+01:00",
            "2026-10-19T11:00:00+01:00",
            "2026-10-19T12:00:00+01:00",
        );
        let case = Case::new(weekly(), moved_s.clone()).exceptions(vec![moved_s]);
        let refused = Err(NotPlanned::Refused(SeriesRefusal::OnlyThisTimeDiffers));
        let titled = |draft: EventPatchDraft| EventPatchDraft {
            summary: Some("Retro".into()),
            ..draft
        };
        assert_eq!(
            case.edit(Scope::All, titled(at("09:00", "2026-10-19"))),
            refused
        );
        let longer = timed_in("2026-10-19", "09:00", "10:30", "Europe/London");
        assert_eq!(case.edit(Scope::All, longer.clone()), refused);
        assert_eq!(case.edit(Scope::All, titled(longer)), refused);
        // Time alone, back on the slot: the series already has it.
        assert_eq!(
            case.edit(Scope::All, at("09:00", "2026-10-19")),
            Err(NotPlanned::Refused(SeriesRefusal::OnlyThisDiffers))
        );
        // An occurrence on its slot with only its own end: a new length for the series wouldn't
        // reach it either (third verification round).
        let resized = moved(
            "2026-10-19T09:00:00+01:00",
            "2026-10-19T09:00:00+01:00",
            "2026-10-19T11:00:00+01:00",
        );
        let case = Case::new(weekly(), resized.clone()).exceptions(vec![resized]);
        assert_eq!(
            case.edit(
                Scope::All,
                timed_in("2026-10-19", "09:00", "12:00", "Europe/London")
            ),
            refused
        );
        assert_eq!(
            case.edit(Scope::All, at("09:00", "2026-10-19")),
            Err(NotPlanned::Refused(SeriesRefusal::OnlyThisDiffers))
        );
    }

    /// Nuuk skips 23:00-24:00 on 27-03-2027, so a 23:30 series' occurrence that day is at 00:30 on
    /// the 28th. The rest of the series can't start there either (fix review, second round).
    #[test]
    fn the_rest_cant_start_where_an_evening_gap_pushes_it_into_the_next_day() {
        let nuuk = "America/Nuuk";
        let late = in_zone(
            master_with(
                &[
                    "RRULE:FREQ=DAILY",
                    "EXDATE;TZID=America/Nuuk:20270405T233000",
                ],
                "2027-03-20T23:30:00-02:00",
                "2027-03-21T00:30:00-02:00",
            ),
            nuuk,
        );
        let s = in_zone(plain("2027-03-28T01:30:00Z"), nuuk);
        let case = Case::new(late, s).exceptions(vec![cancelled("2027-04-06T00:30:00Z")]);
        let refused = Err(NotPlanned::Refused(SeriesRefusal::SplitInGap(day(
            2027, 3, 27,
        ))));
        assert_eq!(case.edit(Scope::Following, title("Retro")), refused);
        assert_eq!(
            case.edit(
                Scope::Following,
                timed_in("2027-03-28", "01:00", "02:00", nuuk)
            ),
            refused
        );
    }

    /// FIX-3: the occurrence moved in Google, since the editor opened, to exactly the drafted time is
    /// still a conflict: the rest of the series hasn't moved.
    #[test]
    fn a_drafted_time_google_already_shows_is_still_checked() {
        let mut case = Case::new(weekly(), monday_19());
        let there = moved(
            "2026-10-19T09:00:00+01:00",
            "2026-10-19T10:00:00+01:00",
            "2026-10-19T11:00:00+01:00",
        );
        case.instance = fresh(there.clone());
        case.exceptions = complete(vec![there]);
        for scope in [Scope::All, Scope::Following] {
            assert_eq!(
                case.edit(scope, at("10:00", "2026-10-19")),
                Err(NotPlanned::Conflict(vec!["start".into(), "end".into()]))
            );
        }
    }

    /// FIX-5: a new length alone sends the end and leaves the rule, its EXDATEs and its exceptions as
    /// they are.
    #[test]
    fn a_new_length_alone_leaves_the_rule_alone() {
        let case = Case::new(weekly(), monday_19()).exceptions(vec![
            cancelled("2026-10-26T09:00:00Z"),
            moved(
                "2026-11-02T09:00:00Z",
                "2026-11-02T14:00:00Z",
                "2026-11-02T15:00:00Z",
            ),
        ]);
        let plan = case
            .edit(
                Scope::All,
                timed_in("2026-10-19", "09:00", "10:30", "Europe/London"),
            )
            .unwrap();
        let sent = body(&plan.steps[0]);
        assert_eq!(sent.keys().collect::<Vec<_>>(), ["end"]);
        assert_eq!(
            sent["end"],
            json!({ "dateTime": "2026-10-05T10:30:00+01:00", "timeZone": "Europe/London" })
        );
        assert_eq!(plan.warnings, vec![SeriesWarning::LengthChanges]);
        assert_eq!(plan.steps[0].undo, None);
        // Its 412 recheck still watches the start and rule the new end is measured from.
        assert!(matches!(
            &plan.steps[0].conflict,
            Recheck::Unchanged(base) if base.contains_key("start") && base.contains_key("recurrence")
        ));
    }

    /// FIX-6: a COUNT split with a new time (COUNT rebased, then retimed), and a delete of the rest of
    /// a COUNT series.
    #[test]
    fn a_count_series_split_with_a_new_time_and_deleted_from_an_occurrence() {
        let counted = master(&["RRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=10"]);
        let fifth = plain("2026-11-02T09:00:00Z");
        let mut case = Case::new(counted, fifth.clone());
        case.instances = Some(Listing {
            items: vec![
                plain("2026-10-05T09:00:00+01:00"),
                plain("2026-10-12T09:00:00+01:00"),
                plain("2026-10-19T09:00:00+01:00"),
                plain("2026-10-26T09:00:00Z"),
                fifth,
            ],
            complete: true,
        });
        let plan = case
            .edit(Scope::Following, at("10:00", "2026-11-02"))
            .unwrap();
        let tail = body(&plan.steps[0]);
        assert_eq!(
            tail["recurrence"],
            json!(["RRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=6"])
        );
        assert_eq!(
            tail["start"],
            json!({ "dateTime": "2026-11-02T10:00:00+00:00", "timeZone": "Europe/London" })
        );
        let delete = case.delete(Scope::Following).unwrap();
        assert_eq!(
            body(&delete.steps[0])["recurrence"],
            json!(["RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261102T085959Z"])
        );
    }

    /// FIX-6: a New York series moved, and split, across its own change of clocks (01-11-2026): the
    /// times are New York's, before and after.
    #[test]
    fn a_new_york_series_moves_in_its_own_zone() {
        let ny = "America/New_York";
        let series = in_zone(
            master_with(
                &["RRULE:FREQ=WEEKLY;BYDAY=MO"],
                "2026-10-05T09:00:00-04:00",
                "2026-10-05T10:00:00-04:00",
            ),
            ny,
        );
        let october = in_zone(plain("2026-10-19T09:00:00-04:00"), ny);
        let plan = Case::new(series.clone(), october)
            .edit(Scope::All, timed_in("2026-10-19", "10:00", "11:00", ny))
            .unwrap();
        assert_eq!(
            body(&plan.steps[0])["start"],
            json!({ "dateTime": "2026-10-05T10:00:00-04:00", "timeZone": ny })
        );
        let november = in_zone(plain("2026-11-02T09:00:00-05:00"), ny);
        let plan = Case::new(series, november)
            .edit(
                Scope::Following,
                timed_in("2026-11-02", "10:00", "11:00", ny),
            )
            .unwrap();
        assert_eq!(
            body(&plan.steps[0])["start"],
            json!({ "dateTime": "2026-11-02T10:00:00-05:00", "timeZone": ny })
        );
        assert_eq!(
            body(&plan.steps[1])["recurrence"],
            json!(["RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261102T135959Z"])
        );
    }
}
