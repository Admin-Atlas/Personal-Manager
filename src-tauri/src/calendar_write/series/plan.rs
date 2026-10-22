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
//!   property, notify none). A 2xx holding the old rule at an etag no copy of the cut went out on
//!   proves the cut not done; a 412, or a 2xx that left Google at the cut's etag, proves nothing by
//!   itself, so C9 GETs again and takes the cut's verdict: the cut's rule is done, `old` at an etag
//!   other than the cut's is proved not done, anything else unknown. Proved not done, the tail
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
use crate::calendar_recur::lines::{self, same_recurrence, Applied};
use crate::calendar_recur::reanchor::DayMap;
use crate::calendar_recur::remap::{exdate_lines, retime, RetimeError, Retimed};
use crate::calendar_recur::spec::spec_of;
use crate::calendar_recur::split::{
    remaining_count, tail_recurrence, truncate_recurrence, Truncation,
};
use crate::calendar_recur::{rfc_instant, Point, RecurError, SeriesStart};

use super::super::attendees::{self, GuestError, Own, Reach};
use super::super::dto::{
    EventPatchDraft, GuestNotice, GuestOp, Notify, ReadOnlyReason, SeenSummary,
};
use super::super::patch::{
    bool_of, build_patch, delete_still_matches, description_is_html, drop_other_kind, is_gone,
    update_check, when_eq, CREATABLE,
};
use super::super::patch::{DeleteCheck, UpdateCheck};
use super::super::plan::{
    delete_event, insert_event, is_plain_segment, patch_event, RequestPlan, VersionFlags,
};
use super::super::time;
use super::diff::{Expect, Removes, SeriesDiff};
use super::rule::{plan_all_rule, plan_split_rule, rule_ask, RuleAsk};
use super::scope::{is_first, zones_changed, Scope};
use super::{
    attendee_set, classify_instance, excluded, google_before, original_of, point_of, Exceptions,
    FreshEvent, InstanceKind, LaterPart, Length, Listing, Master, SeriesRefusal,
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
    /// Where the earliest later part of the series starts, which a new rule mustn't repeat over (C11
    /// critique 1 H2).
    pub later: LaterPart,
    /// The address the account reads this calendar with: the user, never a guest Google emails,
    /// whatever a list PM built calls it (C12 critiques C1-H1, C2-H1).
    pub me: &'a str,
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
    /// The series' start, for reading recurrence lines by what they mean: what it held before the
    /// plan, and what Google holds when a step didn't land.
    pub(super) start: SeriesStart,
    /// The start the series a step patches repeats from once it has landed: a rule rebuilt for every
    /// occurrence starts on its own first day, maybe of another kind or in another zone, and what was
    /// sent is read at it (C11; critique 2 M8). [`SeriesPlan::start`] for every other plan.
    pub(super) rule_start: SeriesStart,
    /// Who Google would email if this ran with Send (R9, A3): `None` when nobody. In the token.
    pub guests: Option<GuestNotice>,
    /// The Send / Don't send answer it was planned with; `None` when nobody answered (C12).
    pub(super) chosen: Option<Notify>,
    /// The calendar's own addresses it counted guests without, for reading Google's copy again on a
    /// 412 the same way (C12a review).
    pub(super) own: Own,
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
    /// The guest changes its body's guest list was built with (C12), so a copy sent again after a 412
    /// builds it on Google's list then ([`SeriesPlan::again_on`]). Empty when it sends no guest list.
    pub(super) guest_ops: Vec<GuestOp>,
    /// Who it reaches on the copy it was planned on, stamped with its `notify`: a resend that would
    /// reach anyone else stops ([`SeriesPlan::resend`]). `None` for an insert or a carry.
    pub(super) reach: Option<Reach>,
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
    /// A new event under PM's own id, minted for this plan and kept for every retry of it:
    /// re-sending the same id normally can't make a second one (Google warns it can't always
    /// catch a repeated id, so the sends stay few), and a 409 means it's there (DS5). A new plan
    /// gets a new id. A split's new series, or a new event ([`SeriesPlan::create`]).
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
                "attendees",
                "guestsCanModify",
                "guestsCanInviteOthers",
                "guestsCanSeeOtherGuests",
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
    /// There at all, under PM's own id: a new event (C10). The id is 128 random bits, so a live copy
    /// under it is this create's, whatever has been changed on it since, and a cancelled one is this
    /// create's too, deleted in Google since: proof it isn't there to land (C10 critique 1 L11). None
    /// proves nothing on its own (DS5).
    Created,
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
    pub fn gone_is_done(&self) -> bool {
        matches!(self, Undo::DeleteInserted { .. })
    }

    /// A 412, `fresh` being Google's copy now: whether to send the undo once more on its etag. A
    /// guest's reply (a room accepts within seconds) moves a new series' etag, and it's still PM's to
    /// delete while it says it was split off the original; a restore meets a change and stops.
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
pub(crate) fn is_tail_of(event: &Value, of: &str, plan: &str) -> bool {
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
    /// (L-R2/L-R3 record which). `complete` false: PM couldn't see them all, so it's "some", raised
    /// even with none seen; part of the warning, so a choice's token tells the two apart.
    ExceptionsMayReset { count: u32, complete: bool },
    /// The series' first occurrence moves.
    StartMoved { from: NaiveDate, to: NaiveDate },
    /// The days the series falls on move, as the occurrence at `from` moves to `to`: measured from
    /// where the occurrence first was, so retiming one that was already moved on its own moves the
    /// series' days too (AS2a). Raised for any move to another day, whatever the rule, a move of
    /// whole weeks included; but not with a new rule that names no day, whose start says it all
    /// (C11). C9 words it from the rule and both dates: the new weekday, day of the
    /// month or date where the rule names one ("every Sunday → every Monday?"), and how far every
    /// occurrence moves wherever that is a week or more (a one-day weekly rule may move a week and a
    /// day) or the rule names no day (daily). A split that keeps its rule raises no `StartMoved`, so
    /// this is all it says; one with a new rule says where that starts too (C11).
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
    /// It stops repeating ("Does not repeat"): only the occurrence on `day` stays, as an event of its
    /// own (C11).
    BecomesOneEvent { day: NaiveDate },
    /// This many occurrences changed on their own fall on days the new rule skips, so their changes
    /// go with them (C11).
    ChangesLost { count: u32 },
}

/// What guests may do: Google applies it to the whole series, like visibility (C12 critiques C1-M7,
/// C2-L11), so never to one occurrence.
const GUEST_PERMISSIONS: &[&str] = &[
    "guestsCanModify",
    "guestsCanInviteOthers",
    "guestsCanSeeOtherGuests",
];

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
pub(super) const TAIL_WATCH: &[&str] = &[
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

/// The original's listed exceptions of `series` from `at` on (by original start, in `zone`), each
/// with what a cleanup cancel watches of it, sorted: what a "Finish" compares a fresh listing with,
/// so it never runs on occurrences changed since (C9 design review M5). By content, not etag, so
/// Google touching them past a new end (P5) isn't read as someone's change (C9a-3b critique M10).
/// One whose original start can't be read is kept too, so a new one always differs.
pub fn marks_from(
    items: &[Value],
    series: &str,
    at: Point,
    zone: Tz,
) -> Vec<(String, Map<String, Value>)> {
    let mut marks: Vec<(String, Map<String, Value>)> = items
        .iter()
        .filter(|item| item.get("recurringEventId").and_then(Value::as_str) == Some(series))
        .filter(|item| {
            original_of(item).is_none_or(|p| p.cmp_in(&at, zone) != std::cmp::Ordering::Less)
        })
        .map(|item| (text(item, "id"), snapshot(item, CANCEL_WATCH)))
        .collect();
    marks.sort_by(|a, b| a.0.cmp(&b.0));
    marks
}

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
    /// A 412 on step `step`, `fresh` being Google's copy now: send again, or not. Asked only once no
    /// copy of the step can land: [`SeriesPlan::verdict`] found it not landed, or found neither the
    /// change nor what was there with every copy refused and Google at an etag none went out on.
    /// Who hears is the user's to say, for what they saw, whatever the step watches (C9a-3a design
    /// review; C12 §6.5 as the critiques' C1-M5 and C2-H3 read it): a step planned to email nobody,
    /// with nobody's Don't send behind it, that would now change a guest's event is a conflict, and
    /// so is one under a Send that would now email anyone else, or not someone it would have: the
    /// save stops, and the next one asks with the numbers Google's copy gives, as a one-off's second
    /// round asks again. Under a Don't send, nobody is emailed whoever appeared.
    pub fn resend(&self, step: usize, fresh: &Value) -> Resend {
        let again = self.again_on(step, fresh);
        let Some(step) = self.steps.get(step) else {
            return Resend::Conflict(Vec::new());
        };
        if is_gone(fresh) {
            return Resend::Gone;
        }
        let guests_changed = Resend::Conflict(vec!["attendees".into()]);
        // R9: who hears is the user's to say, and they said it for no one here.
        if step.notify == Notify::None
            && self.chosen != Some(Notify::None)
            && attendees::reaches_anyone(fresh, &self.own)
        {
            return guests_changed;
        }
        // A guest list goes again built on Google's list now (C12 decision 3), which has to be one
        // the step's guest changes apply to: a whole one, say.
        if again.is_some() && attendees::check(fresh, &step.guest_ops).is_err() {
            return guests_changed;
        }
        // An answer stands only for who it was given for (C12 critiques C1-M5, C2-H3).
        let sent = again.as_ref().unwrap_or(&step.op);
        if step.notify != Notify::None && step.reach != reach_onto(sent, fresh, &self.own) {
            return guests_changed;
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

    /// The write step `step` sends again once [`SeriesPlan::resend`] lets it, `fresh` being Google's
    /// newer copy, when it sends a guest list: that list built again from Google's with the step's
    /// guest changes, so a reply, a comment or extra guests that landed meanwhile goes back as Google
    /// has it (C12 decision 3: on every path the ops apply to Google's list as it is then), the rest
    /// as planned. `None` for a write with no guest list, whose last copy goes again as it was.
    pub(super) fn again_on(&self, step: usize, fresh: &Value) -> Option<Op> {
        let step = self.steps.get(step)?;
        let Op::Patch {
            calendar,
            event,
            etag,
            body,
        } = &step.op
        else {
            return None;
        };
        if !body.contains_key("attendees") {
            return None;
        }
        let mut body = body.clone();
        let list = attendees::apply(fresh.get("attendees"), &step.guest_ops);
        body.insert("attendees".into(), Value::from(list));
        Some(Op::Patch {
            calendar: calendar.clone(),
            event: event.clone(),
            etag: etag.clone(),
            body,
        })
    }

    /// Whether step `step` landed, read from Google's copy (`None` for a 404 or 410): its reply, or a
    /// GET after an answer that may have been applied.
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
            (Shows::Created, Some(v)) if !is_gone(v) => Applied::Yes,
            // Made, then deleted in Google: the run reads it as gone ([`super::run`]).
            (Shows::Created, Some(_)) => Applied::No,
            (Shows::Created, None) => Applied::Unknown,
            (Shows::Fields { .. }, None) if step.gone_is_done => Applied::Yes,
            // The event itself is gone: whatever was sent can't be there.
            (Shows::Fields { .. }, None) => Applied::No,
            (Shows::Fields { sent, old }, Some(v)) => {
                // What was sent is read at the start it repeats from once landed, and what was
                // there at the start it had: never one start for both (C11 critique 2 M8).
                let holds = |fields: &Map<String, Value>, start: &SeriesStart| {
                    fields
                        .iter()
                        .all(|(k, want)| self.same_at(k, Some(want), v.get(k.as_str()), start))
                };
                if holds(sent, &self.rule_start) {
                    Applied::Yes
                } else if !old.is_empty() && holds(old, &self.start) {
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
        // The rule sent read at the start it repeats from once landed, the old one at its own.
        let now = lines_of(fresh.get("recurrence"));
        let verdict = if same_recurrence(&lines_of(Some(sent_rule)), &now, &self.rule_start) {
            Applied::Yes
        } else if same_recurrence(&lines_of(old.get("recurrence")), &now, &self.start) {
            Applied::No
        } else {
            Applied::Unknown
        };
        (start_landed && verdict == Applied::No) || (start_kept && verdict == Applied::Yes)
    }

    /// Whether `fresh` holds what step `step`'s [`Undo::RestoreMaster`] puts back: the times and rule
    /// as they were. Only those: the text the step sent may have landed with them.
    pub fn restored(&self, step: usize, fresh: &Value) -> bool {
        let Some(Step {
            undo: Some(Undo::RestoreMaster { body, .. }),
            ..
        }) = self.steps.get(step)
        else {
            return false;
        };
        !is_gone(fresh)
            && body
                .iter()
                .all(|(k, want)| self.same(k, Some(want), fresh.get(k.as_str())))
    }

    /// What a re-fetch has to show once the run is over, the cleanup cancels in `skipped` (step
    /// indexes) not done: their occurrences are still in Google, so a re-fetch listing them still
    /// shows the write (C8b review R2).
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

    /// The token a choice row carries (DS12): a digest of what the plan does (its scope, its
    /// warnings, and each step's kind, stage and dependency), bound to `bind`, the editor or row it
    /// was offered for and the draft. Never an etag or the new series' id, which differ from one
    /// preview to the next. At send the plan is made again and must give the same token, or the user
    /// is asked again (C9a-3), so what goes to Google is what they chose.
    pub fn choice_digest(&self, bind: &str) -> String {
        use sha2::{Digest, Sha256};
        use std::fmt::Write;
        let mut text = format!("{bind}\u{1f}{:?}", self.scope);
        for warning in &self.warnings {
            let _ = write!(text, "\u{1f}{warning:?}");
        }
        // Who it emails (C12 §6.4): a guest added or taken off in Google between the dialog and the
        // save asks again, with the new numbers. Never the answer, which comes with the save.
        let _ = write!(text, "\u{1f}{:?}", self.guests);
        for step in &self.steps {
            let kind = match step.op {
                Op::Insert { .. } => "insert",
                Op::Patch { .. } => "patch",
                Op::Cancel { .. } => "cancel",
                Op::Delete { .. } => "delete",
                Op::Carry { .. } => "carry",
            };
            let _ = write!(text, "\u{1f}{kind} {:?} {:?}", step.stage, step.requires);
        }
        hex::encode(&Sha256::digest(text.as_bytes())[..8])
    }

    /// The rule and start the series repeats by once this change has run, for the question a move
    /// to another day asks (A29): a split's new series, or the master after "all events". Where the
    /// change moves only the start, as one to a monthly or yearly rule that names no day does, the
    /// rule is the master's own, `recurrence` (C9 design review scope-L1). `None` for "this event",
    /// and for a change to neither.
    pub fn new_rule(&self, recurrence: &[String]) -> Option<(Vec<String>, SeriesStart)> {
        if self.scope == Scope::This {
            return None;
        }
        let body = self.steps.iter().find_map(|s| match (&s.op, s.stage) {
            (Op::Insert { body, .. } | Op::Patch { body, .. }, Stage::Commit) => Some(body),
            _ => None,
        })?;
        let start = match body.get("start") {
            Some(node) => start_of(node)?,
            None if body.contains_key("recurrence") => self.start,
            None => return None,
        };
        let lines = match body.get("recurrence") {
            Some(lines) => lines_of(Some(lines)),
            None => recurrence.to_vec(),
        };
        Some((lines, start))
    }

    /// Whether `fresh`, Google's copy of the new series step `step` inserted, still holds what that
    /// step sent, read as [`SeriesPlan::same`] reads (PM's own notes aside): a change made to it in
    /// Google since (a rename, a move, a place or a guest added) means it's no longer only PM's to
    /// delete (C9a-3b critique M6). Every field an insert may send is compared, sent or not, and
    /// anything it never sends (a call, attachments, a label) reads as a change (review round 2).
    pub fn tail_as_inserted(&self, step: usize, fresh: &Value) -> bool {
        let Some(Op::Insert { body, .. }) = self.steps.get(step).map(|s| &s.op) else {
            return false;
        };
        const NEVER_SENT: &[&str] = &[
            "conferenceData",
            "hangoutLink",
            "attachments",
            "eventLabelId",
        ];
        // Its rule read at its own start, which a new rule may put in another kind or zone (C11).
        let start = body.get("start").and_then(start_of).unwrap_or(self.start);
        // The user's own entry went without `self`, which Google's copy then marks (C12 critique
        // C1-H1): it is the user on both sides, not a guest either side added.
        let own = Own::of(&[], &[fresh]);
        !is_gone(fresh)
            && TAIL_FIELDS.iter().all(|k| match *k {
                "attendees" => {
                    attendees::guest_set_but(body.get(*k), &own)
                        == attendees::guest_set_but(fresh.get(*k), &own)
                }
                _ => self.same_at(k, body.get(*k), fresh.get(*k), &start),
            })
            && NEVER_SENT
                .iter()
                .all(|k| fresh.get(*k).is_none_or(Value::is_null))
    }

    /// The occurrences the new series step `step` inserts leaves out from the start (the original's
    /// deleted ones, as EXDATEs): Google may list them as cancelled occurrences of it, which are no
    /// change anyone made (C9a-3b critique M11).
    pub fn tail_exdates(&self, step: usize) -> Vec<Point> {
        let Some(Op::Insert { body, .. }) = self.steps.get(step).map(|s| &s.op) else {
            return Vec::new();
        };
        match body.get("start").and_then(start_of) {
            Some(start) => excluded(&lines_of(body.get("recurrence")), &start),
            None => Vec::new(),
        }
    }

    /// Where the cleanup cancels among `skipped` would have cancelled.
    pub fn left_behind(&self, skipped: &[usize]) -> Vec<Point> {
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
    /// text-only, which reads any two lists as equal (DS9, G4). A rule is read at the series' start.
    fn same(&self, key: &str, a: Option<&Value>, b: Option<&Value>) -> bool {
        self.same_at(key, a, b, &self.start)
    }

    /// [`SeriesPlan::same`], a rule read at `start`: both sides at one start, the one they repeat
    /// from (C11 critique 2 M8).
    fn same_at(
        &self,
        key: &str,
        a: Option<&Value>,
        b: Option<&Value>,
        start: &SeriesStart,
    ) -> bool {
        let or = |v: Option<&Value>, d: &str| v.and_then(Value::as_str).unwrap_or(d).to_string();
        match key {
            "start" | "end" => when_eq(a, b),
            // A guest's reply isn't a change to the guest list (C8b review EXEC-6).
            "attendees" => attendee_set(a) == attendee_set(b),
            "recurrence" => same_recurrence(&lines_of(a), &lines_of(b), start),
            "status" => or(a, "confirmed") == or(b, "confirmed"),
            "transparency" => or(a, "opaque") == or(b, "opaque"),
            "visibility" => or(a, "default") == or(b, "default"),
            // Flags Google leaves out at their defaults (C12 §6.7): an explicit default against an
            // absent key is no change.
            "guestsCanModify" | "anyoneCanAddSelf" | "privateCopy" => {
                bool_of(a, false) == bool_of(b, false)
            }
            "guestsCanInviteOthers" | "guestsCanSeeOtherGuests" => {
                bool_of(a, true) == bool_of(b, true)
            }
            "summary" | "description" | "location" | "colorId" => or(a, "") == or(b, ""),
            // PM's own notes (a fence, a split's) are bookkeeping, not a change anyone made (C9
            // design review M9).
            "extendedProperties" => super::without_pm_notes(a) == super::without_pm_notes(b),
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

/// A `start` node as a series' start: its day, or its instant in its own zone (A27).
pub(crate) fn start_of(node: &Value) -> Option<SeriesStart> {
    if let Some(day) = node.get("date").and_then(Value::as_str) {
        return NaiveDate::parse_from_str(day, "%Y-%m-%d")
            .ok()
            .map(SeriesStart::AllDay);
    }
    let zone: Tz = node.get("timeZone")?.as_str()?.parse().ok()?;
    let at = DateTime::parse_from_rfc3339(node.get("dateTime")?.as_str()?.trim()).ok()?;
    Some(SeriesStart::Timed {
        local: at.with_timezone(&zone).naive_local(),
        zone,
    })
}

/// A `recurrence` value's lines: none for `null`, `[]` or a value left out alike (C11 critique 1 L10).
fn lines_of(v: Option<&Value>) -> Vec<String> {
    v.and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect()
}

/// The values `fresh` holds for `keys`, absent ones as `null`.
pub(super) fn snapshot(fresh: &Value, keys: &[&str]) -> Map<String, Value> {
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
pub(super) fn own_exceptions<'a>(
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
pub(super) struct SplitCut {
    /// The original's new `recurrence`.
    pub(super) cut: Vec<String>,
    /// Its EXDATEs and RDATEs from the occurrence on, for the rest.
    pub(super) moved: Vec<lines::Line>,
    /// Google's count of what came before, when the rule has a COUNT.
    pub(super) before: Option<u32>,
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
pub(super) fn exceptions_from<'a>(
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

/// The gate reads the occurrence's description; a draft for the series writes the series' own, and
/// would save a formatted one as plain text (C9 design review L7).
pub(super) fn keeps_formatting(
    master_json: &Value,
    draft: &EventPatchDraft,
) -> Result<(), NotPlanned> {
    let formatted = master_json
        .get("description")
        .and_then(Value::as_str)
        .is_some_and(description_is_html);
    if formatted && draft.description.is_some() {
        return Err(SeriesRefusal::FormattedDescription.into());
    }
    Ok(())
}

/// A cleanup cancel of the original's own copy of the occurrence at `original`, which moved to the new
/// series (or was cut off): done when Google already shows it cancelled or gone; re-sent on a 412
/// only while nothing it would throw away has changed (DS10).
pub(super) fn cancel_exception(
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
        guest_ops: Vec::new(),
        reach: None,
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
pub(super) fn cut_step(
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
        guest_ops: Vec::new(),
        reach: None,
    }
}

/// Delete the occurrence (`This`), the series from it on (`Following`), or the whole series (`All`)
/// (T8). `seen` is what the user saw of the occurrence; the delete goes ahead only if that's still
/// what's there (A8, A15). `notify` is the user's Send / Don't send, `None` when nobody answered: a
/// plan that would tell someone then never runs ([`SeriesPlan::needs_notify`]), and its guests say
/// whom it would (C12).
pub fn plan_series_delete(
    scope: Scope,
    input: &SeriesInput,
    seen: &SeenSummary,
    notify: Option<Notify>,
) -> Result<SeriesPlan, NotPlanned> {
    let mut plan = delete_plan(scope, input, seen, notify.unwrap_or(Notify::None))?;
    plan.settle_notify(input, notify, true);
    Ok(plan)
}

fn delete_plan(
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
    // Every arm that deletes the whole series comes through here.
    let whole = |warnings: Vec<SeriesWarning>| -> SeriesPlan {
        SeriesPlan {
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
                guest_ops: Vec::new(),
                reach: None,
            }],
            warnings,
            expect: vec![Expect::Absent {
                series: master.id.clone(),
            }],
            removes: Removes::Series {
                series: master.id.clone(),
            },
            start: master.start,
            rule_start: master.start,
            guests: None,
            chosen: None,
            own: Own::default(),
        }
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
                guest_ops: Vec::new(),
                reach: None,
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
            rule_start: master.start,
            guests: None,
            chosen: None,
            own: Own::default(),
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
                rule_start: master.start,
                guests: None,
                chosen: None,
                own: Own::default(),
            }),
        },
    }
}

/// Where the draft puts the occurrence, when it changes its time: its new start, end and length.
struct NewTime {
    start: Point,
    length: Length,
}

/// The draft's time, if it changes the occurrence's. A move between all-day and timed, or into
/// another zone, never gets here: it rebuilds the series ([`own_rule_rebuild`]).
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

/// The series' own rule to rebuild it from, when `draft` moves the occurrence (`original` in the
/// series) between all-day and timed, or puts its start or end in another zone: the rule's text
/// can't follow either, so every occurrence is rebuilt from it as a new rule rebuilds them (C11
/// §12.4). `None` for any other change, a move included. A rule PM can't read as one of the
/// editor's is refused, and so is a draft on a day other than the occurrence's own in the series,
/// which would need the rule's own days moved too: one the user moved it to, or one it was already
/// moved to on its own and left on.
fn own_rule_rebuild(
    draft: &EventPatchDraft,
    instance: &Value,
    master: &Master,
    original: Point,
) -> Result<Option<RuleAsk>, NotPlanned> {
    let Some(draft_time) = &draft.time else {
        return Ok(None);
    };
    let when = time::resolve_against(draft_time, instance.get("start"), instance.get("end"))?;
    if when_eq(instance.get("start"), Some(&when.start))
        && when_eq(instance.get("end"), Some(&when.end))
    {
        return Ok(None);
    }
    let kind = when.all_day != matches!(master.start, SeriesStart::AllDay(_));
    if !kind && !zones_changed(draft_time, instance) {
        return Ok(None);
    }
    let Some(spec) = spec_of(&master.recurrence, &master.start) else {
        return Err(RetimeError::KindChange.into());
    };
    let zone = master.start.zone();
    let day = time::series_start(draft_time)?.date();
    if day != original.day_in(zone) {
        // Left where it was moved on its own, the user moved no day (C11d review 1).
        let shown = point_of(instance.get("start")).map(|p| p.day_in(zone));
        return Err(if shown == Some(day) {
            SeriesRefusal::KindFromMovedDay
        } else {
            SeriesRefusal::KindAndDay
        }
        .into());
    }
    Ok(Some(RuleAsk::New(spec)))
}

/// Google's `start` and `end` for a series starting at `first` with occurrences `length` long, in
/// the master's own zones.
fn when_nodes(master: &Master, first: Point, length: Length) -> Result<(Value, Value), NotPlanned> {
    when_nodes_in(master.start.zone(), master.end_zone, first, length)
}

/// Google's `start` and `end` for a series starting at `first` with occurrences `length` long, its
/// start in `zone` and its end in `end_zone`: a timed node always names its `timeZone` (G6).
pub(super) fn when_nodes_in(
    zone: Tz,
    end_zone: Tz,
    first: Point,
    length: Length,
) -> Result<(Value, Value), NotPlanned> {
    let timed = |t: DateTime<Utc>, zone: Tz| {
        json!({
            "dateTime": t.with_timezone(&zone).to_rfc3339_opts(SecondsFormat::Secs, false),
            "timeZone": zone.name(),
        })
    };
    let date = |d: NaiveDate| json!({ "date": d.format("%Y-%m-%d").to_string() });
    match (first, length) {
        (Point::At(t), Length::Timed(len)) => Ok((timed(t, zone), timed(t + len, end_zone))),
        (Point::Day(d), Length::AllDay(days)) => {
            let end = d
                .checked_add_signed(chrono::TimeDelta::days(days))
                .ok_or(SeriesRefusal::Unreadable)?;
            Ok((date(d), date(end)))
        }
        _ => Err(RetimeError::KindChange.into()),
    }
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

/// The draft's touched fields, as Google names them: its text and choices, its guest changes as the
/// series' own list (`master_json`'s, Google's entries kept) with them (C12), and what guests may do.
/// A new part takes the guests through [`tail_body`], which leaves their replies behind.
pub(super) fn draft_fields(
    draft: &EventPatchDraft,
    master_json: &Value,
) -> Vec<(&'static str, Value)> {
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
    let ops = draft.guest_ops();
    if !ops.is_empty() {
        let list = attendees::apply(master_json.get("attendees"), ops);
        out.push(("attendees", Value::from(list)));
    }
    for (key, v) in [
        ("guestsCanModify", draft.guests_can_modify),
        ("guestsCanInviteOthers", draft.guests_can_invite_others),
        ("guestsCanSeeOtherGuests", draft.guests_can_see_other_guests),
    ] {
        if let Some(v) = v {
            out.push((key, Value::Bool(v)));
        }
    }
    out
}

impl SeriesPlan {
    /// One new event under `id`, PM's own (`plan::client_event_id`), for creating an event (C10):
    /// the insert step and nothing else, so a lost answer gets the A9 handling every insert gets
    /// (look, wait, look, post again once under the same id; a 409 means look). `None` when the body
    /// holds a key a create may not send (G5), the id isn't one path segment, or the body has no
    /// start PM can read (never an invented one).
    pub fn create(calendar: &str, id: &str, body: Map<String, Value>) -> Option<SeriesPlan> {
        if body.keys().any(|k| !CREATABLE.contains(&k.as_str())) || !is_plain_segment(id) {
            return None;
        }
        let start = body.get("start").and_then(start_of)?;
        Some(SeriesPlan {
            scope: Scope::All,
            steps: vec![Step {
                op: Op::Insert {
                    calendar: calendar.into(),
                    id: id.into(),
                    body,
                },
                // R7: a new event has no guests until C12d, so nobody is emailed.
                notify: Notify::None,
                stage: Stage::Commit,
                requires: None,
                conflict: Recheck::Never,
                shows: Shows::Created,
                gone_is_done: false,
                undo: None,
                guest_ops: Vec::new(),
                reach: None,
            }],
            warnings: Vec::new(),
            expect: Vec::new(),
            removes: Removes::Nothing,
            start,
            rule_start: start,
            guests: None,
            chosen: None,
            own: Own::default(),
        })
    }
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
        rule_start: *start,
        guests: None,
        chosen: None,
        own: Own::default(),
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
/// again (the module's "one plan, one tail id"). `notify` is the user's Send / Don't send, `None`
/// when nobody answered: a plan that would email someone then never runs
/// ([`SeriesPlan::needs_notify`]), and its guests say whom it would (C12).
pub fn plan_series_edit(
    scope: Scope,
    input: &SeriesInput,
    draft: &EventPatchDraft,
    tail_id: &str,
    notify: Option<Notify>,
) -> Result<SeriesPlan, NotPlanned> {
    let mut plan = edit_plan(scope, input, draft, tail_id, notify.unwrap_or(Notify::None))?;
    plan.keep_guest_ops(draft.guest_ops());
    plan.settle_notify(input, notify, false);
    Ok(plan)
}

fn edit_plan(
    scope: Scope,
    input: &SeriesInput,
    draft: &EventPatchDraft,
    tail_id: &str,
    notify: Notify,
) -> Result<SeriesPlan, NotPlanned> {
    let (master, original) = prepare(input)?;
    guests_ready(scope, input, draft)?;
    match scope {
        Scope::This => plan_this(input, &master, draft, notify),
        Scope::All => plan_all(input, &master, original, draft, notify),
        Scope::Following if is_first(&master, input.fresh.instance) => {
            plan_all(input, &master, original, draft, notify)
        }
        Scope::Following => match cut_at(&master, input, original)? {
            None => plan_all(input, &master, original, draft, notify),
            Some(cut) => {
                // The new part copies the series' guest list, which Google cut short would copy part
                // of (C12 critique C1): refused, whatever the draft changes.
                if attendees::omitted(input.fresh.master.json()) {
                    return Err(SeriesRefusal::Guest(GuestError::Omitted).into());
                }
                plan_split(input, &master, original, cut, draft, tail_id, notify)
            }
        },
    }
}

/// The draft's guest changes, checked on the copy they go to: the occurrence for "This event", the
/// series for the rest (C12 §6.1), whose guest list the occurrence the user changed them on must
/// have, or they'd be applied to a list the user never saw (critique C2-M5).
fn guests_ready(
    scope: Scope,
    input: &SeriesInput,
    draft: &EventPatchDraft,
) -> Result<(), NotPlanned> {
    let ops = draft.guest_ops();
    if ops.is_empty() {
        return Ok(());
    }
    let (instance, master_json) = (input.fresh.instance.json(), input.fresh.master.json());
    if scope == Scope::This {
        attendees::check(instance, ops).map_err(SeriesRefusal::Guest)?;
        return Ok(());
    }
    attendees::check(master_json, ops).map_err(SeriesRefusal::Guest)?;
    let list = |v: &Value| attendees::guest_set(v.get("attendees"));
    if list(instance) != list(master_json) {
        return Err(SeriesRefusal::GuestsDifferHere.into());
    }
    Ok(())
}

/// Google's copy of `event` as the plan read it: the series, the occurrence, or one of the series'
/// listed occurrences. `None` for an id PM didn't read.
fn json_of<'a>(input: &SeriesInput<'a>, event: &str) -> Option<&'a Value> {
    let is = |v: &&Value| v.get("id").and_then(Value::as_str) == Some(event);
    [input.fresh.master.json(), input.fresh.instance.json()]
        .into_iter()
        .find(is)
        .or_else(|| input.exceptions.items.iter().find(is))
}

impl SeriesPlan {
    /// It would email someone and nobody answered: it must not run (R9).
    pub fn needs_notify(&self) -> bool {
        self.guests.is_some() && self.chosen.is_none()
    }

    /// The body of the new event it inserts, if it inserts one.
    fn inserted(&self) -> Option<&Map<String, Value>> {
        self.steps.iter().find_map(|s| match &s.op {
            Op::Insert { body, .. } => Some(body),
            _ => None,
        })
    }

    /// Stamp each step's `sendUpdates` from the user's answer: `chosen` where the step reaches
    /// someone, `none` where it reaches nobody (R7), whatever was chosen; and say who the plan
    /// reaches (A3). Every step, cleanup included, takes the answer until probe L-R13 shows guests'
    /// copies of a carried or cancelled occurrence follow the organiser's anyway (A3); a follow-up
    /// may then send `none` there. `deletes`: a delete's plan, whose writes all cancel what they reach
    /// (a cut is a PATCH; C12 critiques C1-M1, C2-M1). Every public planner ends here.
    fn settle_notify(&mut self, input: &SeriesInput, chosen: Option<Notify>, deletes: bool) {
        let own = Own::of(
            &[input.calendar, input.me],
            &[input.fresh.master.json(), input.fresh.instance.json()],
        );
        let reaches: Vec<Reach> = self
            .steps
            .iter()
            .map(|step| self.reach_of(step, input, &own))
            .collect();
        for (step, reach) in self.steps.iter_mut().zip(&reaches) {
            step.notify = match reach.notice() {
                Some(_) => chosen.unwrap_or(Notify::None),
                None => Notify::None,
            };
            step.reach = written(&step.op)
                .and_then(|event| json_of(input, event))
                .and_then(|json| reach_onto(&step.op, json, &own));
        }
        let mut all = reaches
            .into_iter()
            .reduce(Reach::merge)
            .unwrap_or_else(Reach::nobody);
        // Those a new part leaves off are taken off, not told of an update (C12 critique C2-M7).
        if let Some(body) = self.inserted() {
            all = all.taking_off(attendees::left_off(input.fresh.master.json(), body, &own));
        }
        self.guests = all.cancelling(deletes).notice();
        self.chosen = chosen;
        self.own = own;
    }

    /// Keep `ops`, the draft's guest changes, on each write whose body sends a guest list: every such
    /// list was built with them (C12).
    fn keep_guest_ops(&mut self, ops: &[GuestOp]) {
        for step in &mut self.steps {
            if matches!(&step.op, Op::Patch { body, .. } if body.contains_key("attendees")) {
                step.guest_ops = ops.to_vec();
            }
        }
    }

    /// Who one step reaches, from the copy it writes onto (C12 §6.2). A write to the series itself
    /// is "at least" where PM couldn't see every occurrence, or (for every occurrence) where one
    /// has a guest list of its own: whether Google emails those guests too is unprobed (INFERRED,
    /// probe L8-11).
    fn reach_of(&self, step: &Step, input: &SeriesInput, own: &Own) -> Reach {
        let unseen = || Reach::nobody().short();
        let (event, reach) = match &step.op {
            Op::Insert { body, .. } => return Reach::of_insert(body, own),
            // Onto an occurrence of the new part, whose guests are the insert's.
            Op::Carry { body, .. } => {
                return match self.inserted() {
                    Some(new) => Reach::of_carry(new, body, own),
                    None => unseen(),
                }
            }
            Op::Patch { event, .. } | Op::Cancel { event, .. } | Op::Delete { event, .. } => {
                match json_of(input, event).and_then(|json| reach_onto(&step.op, json, own)) {
                    Some(reach) => (event, reach),
                    None => return unseen(),
                }
            }
        };
        let master_json = input.fresh.master.json();
        if master_json.get("id").and_then(Value::as_str) != Some(event.as_str()) || !reach.shared()
        {
            return reach;
        }
        let series_list = attendees::guest_set(master_json.get("attendees"));
        let own_lists = self.scope == Scope::All
            && input
                .exceptions
                .items
                .iter()
                .filter(|item| item.get("recurringEventId") == master_json.get("id"))
                .filter(|item| !is_gone(item))
                .any(|item| attendees::guest_set(item.get("attendees")) != series_list);
        if !input.exceptions.complete || own_lists {
            reach.short()
        } else {
            reach
        }
    }
}

/// The event `op` writes, when Google has it before the write: never an insert's or a carry's.
fn written(op: &Op) -> Option<&str> {
    match op {
        Op::Patch { event, .. } | Op::Cancel { event, .. } | Op::Delete { event, .. } => {
            Some(event)
        }
        Op::Insert { .. } | Op::Carry { .. } => None,
    }
}

/// Who `op` reaches written onto `json`, Google's copy of the event it writes. `None` for an insert
/// or a carry, whose event Google doesn't have before it.
fn reach_onto(op: &Op, json: &Value, own: &Own) -> Option<Reach> {
    match op {
        Op::Patch { body, .. } => Some(Reach::of_patch(json, body, own)),
        Op::Cancel { .. } | Op::Delete { .. } => Some(Reach::of_delete(json, own)),
        Op::Insert { .. } | Op::Carry { .. } => None,
    }
}

fn plan_this(
    input: &SeriesInput,
    master: &Master,
    draft: &EventPatchDraft,
    notify: Notify,
) -> Result<SeriesPlan, NotPlanned> {
    let instance = input.fresh.instance.json();
    // A new rule is the whole series' (scope.rs refuses the row; this is the planner's own guard). The
    // rule the series already has is no change of rule, as the rows read it (C11 critique 1 L12).
    if rule_ask(draft, master)?.is_some() {
        return Err(SeriesRefusal::RuleIsSeriesWide.into());
    }
    let body = build_patch(instance, draft)?;
    if body.is_empty() {
        return Err(NotPlanned::NoChange);
    }
    if body.contains_key("visibility") {
        return Err(SeriesRefusal::VisibilityIsSeriesWide.into());
    }
    if GUEST_PERMISSIONS.iter().any(|k| body.contains_key(*k)) {
        return Err(SeriesRefusal::GuestPermissionsAreSeriesWide.into());
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
            guest_ops: Vec::new(),
            reach: None,
        }],
        warnings: Vec::new(),
        expect: Vec::new(),
        removes: Removes::Nothing,
        start: master.start,
        rule_start: master.start,
        guests: None,
        chosen: None,
        own: Own::default(),
    })
}

fn plan_all(
    input: &SeriesInput,
    master: &Master,
    original: Point,
    draft: &EventPatchDraft,
    notify: Notify,
) -> Result<SeriesPlan, NotPlanned> {
    // A new rule, or none: the series is rebuilt from it (C11). So is a move between all-day and
    // timed, or into another zone, from its own rule.
    if let Some(ask) = rule_ask(draft, master)? {
        return plan_all_rule(input, master, original, draft, ask, notify);
    }
    if let Some(ask) = own_rule_rebuild(draft, input.fresh.instance.json(), master, original)? {
        // The rebuild starts on the first day the rule makes: a first occurrence the rule doesn't
        // make would go, and with "after n times" one more would come at the end, on a save that
        // changed no day. A move refuses the same series (RM14; C11d review 1), in words of its own:
        // here nothing was moved (C11d review 2).
        let parsed = lines::parse(&master.recurrence);
        let (_, facts) = lines::series_rule(&parsed).map_err(RecurError::TextOnly)?;
        if !is_occurrence(&facts, &master.start, master.first)? {
            let first = master.first.day_in(master.start.zone());
            return Err(SeriesRefusal::KindStartNotOnRule(first).into());
        }
        return plan_all_rule(input, master, original, draft, ask, notify);
    }
    let (instance, master_json) = (input.fresh.instance.json(), input.fresh.master.json());
    input.opened.master.ok_or(SeriesRefusal::NoBase)?;
    keeps_formatting(master_json, draft)?;
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
    all_plan(input, master, body, warnings, undo, notify, master.start)
}

/// The one PATCH of the series' master that "all events" sends, `body` being what it changes: what a
/// 412 rechecks and what was changed in Google since the editor opened (R2), the warning for the
/// occurrences changed on their own, after `warnings`, and `undo`. `rule_start`: the start it repeats
/// from once it lands ([`SeriesPlan::rule_start`]).
pub(super) fn all_plan(
    input: &SeriesInput,
    master: &Master,
    body: Map<String, Value>,
    mut warnings: Vec<SeriesWarning>,
    undo: Option<Undo>,
    notify: Notify,
    rule_start: SeriesStart,
) -> Result<SeriesPlan, NotPlanned> {
    let master_json = input.fresh.master.json();
    let opened = input.opened.master.ok_or(SeriesRefusal::NoBase)?;
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
    // A listing PM couldn't see all of may hold more: say "some", even with none seen (C9a-2 review).
    if reset > 0 || !input.exceptions.complete {
        warnings.push(SeriesWarning::ExceptionsMayReset {
            count: u32::try_from(reset).unwrap_or(u32::MAX),
            complete: input.exceptions.complete,
        });
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
            guest_ops: Vec::new(),
            reach: None,
        }],
        warnings,
        expect: Vec::new(),
        removes: Removes::Nothing,
        start: master.start,
        rule_start,
        guests: None,
        chosen: None,
        own: Own::default(),
    })
}

/// The original starts of the series' cancelled occurrences, from a complete listing.
pub(super) fn cancelled_points(
    master: &Master,
    exceptions: &Exceptions,
) -> Result<Vec<Point>, NotPlanned> {
    if !exceptions.complete {
        return Err(SeriesRefusal::Incomplete.into());
    }
    Ok(own_exceptions(master, exceptions)
        .filter(|i| i.get("status").and_then(Value::as_str) == Some("cancelled"))
        .filter_map(original_of)
        .collect())
}

/// A conflict when the occurrence no longer holds, on `keys`, what the user opened (R2).
pub(super) fn unchanged_since_opened(
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
    cut: SplitCut,
    draft: &EventPatchDraft,
    tail_id: &str,
    notify: Notify,
) -> Result<SeriesPlan, NotPlanned> {
    // A new rule, or none: the rest is rebuilt from it (C11). So is a move between all-day and
    // timed, or into another zone, from the series' own rule, whose "after n times" is then rebased
    // on what is left of it, since the user didn't set it.
    if let Some(ask) = rule_ask(draft, master)? {
        return plan_split_rule(input, master, at, cut, draft, tail_id, ask, notify);
    }
    if let Some(ask) = own_rule_rebuild(draft, input.fresh.instance.json(), master, at)? {
        return plan_split_rule(input, master, at, cut, draft, tail_id, ask, notify);
    }
    let SplitCut { cut, moved, before } = cut;
    let (instance, master_json) = (input.fresh.instance.json(), input.fresh.master.json());
    let opened = input.opened.master.ok_or(SeriesRefusal::NoBase)?;
    keeps_formatting(master_json, draft)?;
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
    let rest_start = rest_start(master, at)?;
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
    let touched = draft_fields(draft, master_json);
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

    let nodes = when_nodes(master, first, length)?;
    let body = tail_body(master, master_json, at, &touched, nodes, Some(tail_lines));
    warnings.extend(not_carried(master_json));
    warnings.push(SeriesWarning::NewIdentity);
    if input.sibling {
        warnings.push(SeriesWarning::SplitSibling);
    }
    split_unchanged_since_opened(master, input, opened, &touched)?;
    let mut steps = vec![
        insert_step(input, master, tail_id, body, notify),
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
                guest_ops: Vec::new(),
                reach: None,
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
        rule_start: master.start,
        guests: None,
        chosen: None,
        own: Own::default(),
    })
}

/// Where the rest of a series split at `at` repeats from: the series' own wall time on that
/// occurrence's day. On a day the clocks skip that time, the occurrence sits an hour off it, and the
/// rest can't start there: it would repeat an hour late, and every EXDATE after it would name nothing
/// (C8b fix review).
pub(super) fn rest_start(master: &Master, at: Point) -> Result<SeriesStart, NotPlanned> {
    let zone = master.start.zone();
    match (master.start, at) {
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
            Ok(start)
        }
        (SeriesStart::AllDay(_), Point::Day(d)) => Ok(SeriesStart::AllDay(d)),
        _ => Err(RetimeError::KindChange.into()),
    }
}

/// A split's new series, or the one event a split that stops repeating makes (C11): the original's
/// own fields (from Google's copy, never the mirror), its guests (the draft's list when it changes
/// them), `fields` over them, `start` and `end`, its `recurrence` if it repeats, and private notes of
/// where it came from and of this plan, so its insert can't be read off another plan's
/// (`Shows::Tail`). Its guests are invited anew ([`attendees::for_new_event`]: who, their name,
/// optional, room and extra guests, never a reply or a comment).
pub(super) fn tail_body(
    master: &Master,
    master_json: &Value,
    at: Point,
    fields: &[(&str, Value)],
    (start, end): (Value, Value),
    recurrence: Option<Vec<String>>,
) -> Map<String, Value> {
    let mut body: Map<String, Value> = TAIL_COPY
        .iter()
        .filter_map(|k| master_json.get(*k).map(|v| ((*k).to_string(), v.clone())))
        .collect();
    let guests = fields
        .iter()
        .find(|(k, _)| *k == "attendees")
        .map(|(_, v)| v)
        .or_else(|| master_json.get("attendees"));
    if let Some(guests) = guests.and_then(Value::as_array) {
        let guests: Vec<Value> = guests.iter().filter_map(attendees::for_new_event).collect();
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
            // And the occurrence it was split at, which an uncut original still holds whatever day
            // the new part was moved to: what the overlap backstop measures from (review round 6).
            private.insert("pmSplitAt".into(), Value::from(super::point_text(at)));
        }
    }
    body.insert("extendedProperties".into(), properties);
    for (k, v) in fields.iter().filter(|(k, _)| *k != "attendees") {
        body.insert((*k).to_string(), v.clone());
    }
    body.insert("start".into(), start);
    body.insert("end".into(), end);
    if let Some(lines) = recurrence {
        body.insert("recurrence".into(), Value::from(lines));
    }
    let plan = plan_digest(&body);
    if let Some(private) = body
        .get_mut("extendedProperties")
        .and_then(|p| p.get_mut("private"))
        .and_then(Value::as_object_mut)
    {
        private.insert("pmSplitPlan".into(), Value::from(plan));
    }
    body
}

/// The new part doesn't take the original's attachments, video call or label: the warning, when it
/// names one.
pub(super) fn not_carried(master_json: &Value) -> Option<SeriesWarning> {
    let warning = SeriesWarning::NotCarried {
        attachments: master_json
            .get("attachments")
            .and_then(Value::as_array)
            .is_some_and(|a| !a.is_empty()),
        conference: master_json.get("conferenceData").is_some()
            || master_json.get("hangoutLink").is_some(),
        label: master_json.get("eventLabelId").is_some(),
    };
    let none = SeriesWarning::NotCarried {
        attachments: false,
        conference: false,
        label: false,
    };
    (warning != none).then_some(warning)
}

/// Nothing a split sends may have changed in Google since the editor opened (R2): every field the new
/// part copies, on the series `opened` as the editor kept it, and the draft's `touched` fields on the
/// occurrence.
pub(super) fn split_unchanged_since_opened(
    master: &Master,
    input: &SeriesInput,
    opened: &FreshEvent,
    touched: &[(&str, Value)],
) -> Result<(), NotPlanned> {
    let watched: Vec<String> = TAIL_WATCH.iter().map(|k| (*k).to_string()).collect();
    let changed = changed_since_opened(
        &master.start,
        opened.json(),
        input.fresh.master.json(),
        &watched,
    );
    if !changed.is_empty() {
        return Err(NotPlanned::Conflict(changed));
    }
    let instance_keys: Vec<String> = touched.iter().map(|(k, _)| (*k).to_string()).collect();
    let changed = changed_since_opened(
        &master.start,
        input.opened.instance.json(),
        input.fresh.instance.json(),
        &instance_keys,
    );
    if !changed.is_empty() {
        return Err(NotPlanned::Conflict(changed));
    }
    Ok(())
}

/// The insert of a split's new part under `tail_id`, PM's own id: proved by its notes, undone by
/// deleting it while it's still PM's.
pub(super) fn insert_step(
    input: &SeriesInput,
    master: &Master,
    tail_id: &str,
    body: Map<String, Value>,
    notify: Notify,
) -> Step {
    let plan = body
        .get("extendedProperties")
        .and_then(|p| p.pointer("/private/pmSplitPlan"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
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
        guest_ops: Vec::new(),
        reach: None,
    }
}

/// The last occurrence the original keeps before `at`, a split's occurrence: its rule's latest that
/// isn't deleted (an EXDATE, or among `cancelled`), else its start, which RFC 5545 and Google count
/// even off its own rule. The rest can't start on that day or before (C8b reviews ADV-9). An extra
/// date (RDATE) is an occurrence of its own, which the rest's rule can't double, so it doesn't count.
pub(super) fn last_kept(
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

    fn body(step: &Step) -> &Map<String, Value> {
        step.op.body().unwrap()
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
                plan_series_delete(scope, &case.input(), &seen_of(&case.opened_instance), None),
                Err(NotPlanned::Conflict(vec!["start".into(), "end".into()]))
            );
        }
        case.instance = fresh(cancelled("2026-10-19T09:00:00+01:00"));
        assert_eq!(
            plan_series_delete(
                Scope::This,
                &case.input(),
                &seen_of(&case.opened_instance),
                None
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
                "private": {
                    "pmSplitOf": "abc",
                    "pmSplitAt": "2026-10-19T08:00:00+00:00",
                    "pmSplitPlan": note,
                }
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
        // A3: every step, and every undo, sent with the one choice where it reaches someone; on a
        // series nobody else is on, `none`, whatever was chosen (R7, C12).
        let notify = Notify::All;
        let alone = plan_series_edit(
            Scope::Following,
            &case.input(),
            &at("10:00", "2026-10-19"),
            TAIL,
            Some(notify),
        )
        .unwrap();
        assert!(alone.guests.is_none());
        assert!(alone.steps.iter().all(|s| s.notify == Notify::None));
        let guest = |event: &Value| {
            if is_gone(event) {
                event.clone()
            } else {
                guests_on_it(event.clone(), &["a@example.com"])
            }
        };
        let invited = Case::new(guest(case.master.json()), guest(&monday_19()))
            .exceptions(case.exceptions.items.iter().map(guest).collect());
        let plan = plan_series_edit(
            Scope::Following,
            &invited.input(),
            &at("10:00", "2026-10-19"),
            TAIL,
            Some(notify),
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
        assert!(plan.warnings.contains(&SeriesWarning::ExceptionsMayReset {
            count: 1,
            complete: true
        }));
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
            plan_series_edit(Scope::All, &input, &title("Retro"), TAIL, None),
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

    /// G5, C12: a series with guests plans like any other and says who it would email, whatever
    /// the occurrence shows (C12 lifted the gate's refusal). An occurrence whose guests were removed
    /// on its own is still a change PM can't carry.
    #[test]
    fn a_series_with_guests_plans_and_counts_them() {
        let no_guests_here = monday_19();
        let case = Case::new(with_guests(), no_guests_here);
        let two = |cancelled: bool| GuestNotice {
            updated: 2,
            cancelled,
            people: 2,
            emails: 2,
            complete: true,
            ..GuestNotice::default()
        };
        let all = case.edit(Scope::All, title("Retro")).unwrap();
        assert!(all.guests == Some(two(false)), "a and b get the update");
        assert!(all.needs_notify(), "unanswered, it may not run");
        let gone = case.delete(Scope::All).unwrap();
        assert!(gone.guests == Some(two(true)), "a and b are told");
        let split = case.edit(Scope::Following, title("Retro")).unwrap();
        assert!(split.guests.is_some() && split.needs_notify());
        // Only the user on the list isn't a guest.
        let just_me = with(
            weekly(),
            "attendees",
            json!([{ "email": "me@example.com", "self": true }]),
        );
        let alone = Case::new(just_me, monday_19())
            .edit(Scope::All, title("Retro"))
            .unwrap();
        assert!(alone.guests.is_none() && !alone.needs_notify());
        let solo = with(
            plain("2026-10-26T09:00:00Z"),
            "summary",
            json!("Solo review"),
        );
        let case = Case::new(with_guests(), monday_19()).exceptions(vec![solo]);
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
        let mut case = Case::new(with_guests(), monday_19());
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
        // Planned with Send: an unanswered plan would meet any guest as one nobody chose to tell.
        let plan = case
            .edit_told(Scope::Following, title("Retro"), Some(Notify::All))
            .unwrap();
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

    /// C9 design review M9: a fence "Remove the new series" wrote since the editor opened is PM's own
    /// note, never a change in Google; anyone else's private property still is.
    #[test]
    fn pms_own_notes_on_the_series_are_no_conflict() {
        let mut case = Case::new(weekly(), monday_19());
        let fenced = with(
            weekly(),
            "extendedProperties",
            json!({ "private": { "pmFence": "d \"m1\"" } }),
        );
        // Opened before the fence, planned on Google's copy after it.
        case.master = fresh(with(fenced, "etag", json!("\"m2\"")));
        let plan = case.edit(Scope::Following, title("Retro")).unwrap();
        // And the cut's 412 rule reads a copy without the note the same way.
        assert_eq!(plan.resend(1, &weekly()), Resend::Proceed);
        let theirs = with(
            weekly(),
            "extendedProperties",
            json!({ "private": { "pmFence": "d", "app": "x" } }),
        );
        assert_eq!(
            plan.resend(1, &theirs),
            Resend::Conflict(vec!["extendedProperties".into()])
        );
        case.master = fresh(theirs);
        assert_eq!(
            case.edit(Scope::Following, title("Retro")),
            Err(NotPlanned::Conflict(vec!["extendedProperties".into()]))
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
            .any(|w| matches!(w, SeriesWarning::ExceptionsMayReset { .. })));
    }

    /// G7: an end moved into another zone is a zone change for the series rows, as a start is: every
    /// occurrence, or the rest, is rebuilt from the rule with its end there (C11 §12.4).
    #[test]
    fn an_end_in_another_zone_rebuilds_every_occurrence_with_it() {
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
        let all = case.edit(Scope::All, draft.clone()).unwrap();
        let sent = body(&all.steps[0]);
        assert_eq!(
            sent["end"],
            json!({ "dateTime": "2026-10-05T12:00:00+02:00", "timeZone": "Europe/Paris" })
        );
        assert_eq!(
            sent["start"],
            json!({ "dateTime": "2026-10-05T10:00:00+01:00", "timeZone": "Europe/London" })
        );
        let following = case.edit(Scope::Following, draft).unwrap();
        assert_eq!(
            body(&following.steps[0])["end"],
            json!({ "dateTime": "2026-10-19T12:00:00+02:00", "timeZone": "Europe/Paris" })
        );
    }

    /// The weekly standup from Monday 05-10 to 30-11, its 02-11 occurrence deleted by an EXDATE and
    /// its 26-10 one cancelled.
    fn standup_to_november() -> Case {
        Case::new(
            master(&[
                "RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261130T235959Z",
                "EXDATE;TZID=Europe/London:20261102T090000",
            ]),
            monday_19(),
        )
        .exceptions(vec![cancelled("2026-10-26T09:00:00Z")])
    }

    /// C11 §12.4: every occurrence from timed to all-day is the series rebuilt from its own rule, in
    /// dates: its first day, its end date, and the occurrences deleted from it, as dates. A move of
    /// kind alone moves no day, so it asks nothing about one.
    #[test]
    fn a_kind_change_on_all_rebuilds_from_the_series_rule() {
        let draft = EventPatchDraft {
            time: Some(TimeDraft::AllDay {
                first_day: "2026-10-19".into(),
                last_day: "2026-10-19".into(),
            }),
            ..Default::default()
        };
        let plan = standup_to_november().edit(Scope::All, draft).unwrap();
        let sent = body(&plan.steps[0]);
        assert_eq!(
            sent["start"],
            json!({ "date": "2026-10-05", "dateTime": null, "timeZone": null })
        );
        assert_eq!(
            sent["end"],
            json!({ "date": "2026-10-06", "dateTime": null, "timeZone": null })
        );
        assert_eq!(
            sent["recurrence"],
            json!([
                "RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261130",
                "EXDATE;VALUE=DATE:20261026,20261102"
            ])
        );
        assert!(!sent.contains_key("summary"));
        assert!(plan.rule_start == SeriesStart::AllDay(day(2026, 10, 5)));
        assert!(!plan.warnings.iter().any(|w| matches!(
            w,
            SeriesWarning::RepeatDayChanges { .. } | SeriesWarning::StartMoved { .. }
        )));
        assert!(plan.warnings.contains(&SeriesWarning::LengthChanges));
        assert!(matches!(
            plan.steps[0].undo,
            Some(Undo::RestoreMaster { .. })
        ));
    }

    /// Into another zone, the end date's UNTIL is that day's last second there, in UTC.
    #[test]
    fn a_zone_change_on_all_rewrites_the_until_in_utc() {
        let draft = EventPatchDraft {
            time: Some(TimeDraft::Timed {
                start_date: "2026-10-19".into(),
                start_time: "09:00".into(),
                start_zone: "America/New_York".into(),
                end_date: "2026-10-19".into(),
                end_time: "10:00".into(),
                end_zone: "America/New_York".into(),
            }),
            ..Default::default()
        };
        let plan = standup_to_november().edit(Scope::All, draft).unwrap();
        let sent = body(&plan.steps[0]);
        assert_eq!(
            sent["start"],
            json!({ "dateTime": "2026-10-05T09:00:00-04:00", "timeZone": "America/New_York" })
        );
        let rule = sent["recurrence"][0].as_str().unwrap();
        assert_eq!(rule, "RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261201T045959Z");
        // The deleted days stay deleted, at the new time (New York's clocks go back on 01-11).
        assert_eq!(
            sent["recurrence"][1],
            "EXDATE:20261026T130000Z,20261102T140000Z"
        );
    }

    /// "This and following" into another zone: the rest rebuilt from the series' rule there, its
    /// "after 10 times" rebased on what is left of it, since the user didn't set it (C11 §12.4).
    #[test]
    fn a_zone_change_on_following_rebuilds_the_new_part_and_rebases_its_count() {
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
        let draft = EventPatchDraft {
            time: Some(TimeDraft::Timed {
                start_date: "2026-11-02".into(),
                start_time: "09:00".into(),
                start_zone: "America/New_York".into(),
                end_date: "2026-11-02".into(),
                end_time: "10:00".into(),
                end_zone: "America/New_York".into(),
            }),
            ..Default::default()
        };
        let plan = case.edit(Scope::Following, draft).unwrap();
        let insert = body(&plan.steps[0]);
        assert_eq!(
            insert["recurrence"],
            json!(["RRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=6"])
        );
        assert_eq!(
            insert["start"],
            json!({ "dateTime": "2026-11-02T09:00:00-05:00", "timeZone": "America/New_York" })
        );
        assert_eq!(
            body(&plan.steps[1])["recurrence"],
            json!(["RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261102T085959Z"])
        );
    }

    /// A rule PM can read but can't write back as one of the editor's (here, more times than Google
    /// allows a new one) can't be rebuilt, so the change of kind is refused in words.
    #[test]
    fn a_kind_change_on_a_custom_rule_is_refused_in_words() {
        let case = Case::new(
            master(&["RRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=800"]),
            monday_19(),
        );
        let draft = EventPatchDraft {
            time: Some(TimeDraft::AllDay {
                first_day: "2026-10-19".into(),
                last_day: "2026-10-19".into(),
            }),
            ..Default::default()
        };
        let refused = case.edit(Scope::All, draft);
        assert!(
            refused
                == Err(NotPlanned::Refused(SeriesRefusal::Retime(
                    RetimeError::KindChange
                )))
        );
        assert_eq!(
            SeriesRefusal::Retime(RetimeError::KindChange).to_string(),
            "PM can only change every occurrence between all-day and timed, or into another time \
             zone, when it can read how the event repeats. Change this occurrence on its own."
        );
    }

    /// Weekly on Monday at 01:00 in London, which is Sunday evening in New York. Put in New York at
    /// 01:00 on the same Monday, it stays on Mondays, for every occurrence and from this one on: the
    /// day the series' own rule names is the one on the clock, wherever the rebuild reads days from
    /// (C11c review). No day moved, so nothing asks to move every occurrence a day or says the
    /// series' start moves (C11d review 4), nor, made "Does not repeat", that the one kept moves
    /// (C11d review 5). Kept at the same moment, Sunday in New York, its day
    /// moves: refused in words.
    #[test]
    fn a_zone_change_past_midnight_keeps_the_rules_own_day() {
        let late = master_with(
            &["RRULE:FREQ=WEEKLY;BYDAY=MO"],
            "2026-10-05T01:00:00+01:00",
            "2026-10-05T02:00:00+01:00",
        );
        let case = Case::new(late, plain("2026-10-19T01:00:00+01:00"));
        let monday = timed_in("2026-10-19", "01:00", "02:00", "America/New_York");
        let all = case.edit(Scope::All, monday.clone()).unwrap();
        assert_eq!(
            body(&all.steps[0])["start"],
            json!({ "dateTime": "2026-10-05T01:00:00-04:00", "timeZone": "America/New_York" })
        );
        assert!(!all.warnings.iter().any(|w| matches!(
            w,
            SeriesWarning::RepeatDayChanges { .. } | SeriesWarning::StartMoved { .. }
        )));
        // Moved on to Tuesday there too and made "Weekly on Tuesday", a day moves: both are said,
        // in the series' own days, Monday to Tuesday and 05-10 to 06-10.
        let tuesday = EventPatchDraft {
            recurrence: Some(crate::calendar_write::dto::RepeatDraft::Rule {
                rule: crate::calendar_recur::wire::RepeatRule {
                    pattern: crate::calendar_recur::wire::RepeatPattern::Weekly {
                        days: vec![crate::calendar_recur::wire::DayCode::Tue],
                    },
                    interval: 1,
                    ends: crate::calendar_recur::wire::RepeatEnds::Never,
                },
            }),
            ..timed_in("2026-10-20", "01:00", "02:00", "America/New_York")
        };
        let moved = case.edit(Scope::All, tuesday).unwrap();
        assert_eq!(
            body(&moved.steps[0])["start"],
            json!({ "dateTime": "2026-10-06T01:00:00-04:00", "timeZone": "America/New_York" })
        );
        let day = |d| NaiveDate::from_ymd_opt(2026, 10, d).unwrap();
        assert!(moved.warnings.contains(&SeriesWarning::RepeatDayChanges {
            from: day(19),
            to: day(20)
        }));
        assert!(moved.warnings.contains(&SeriesWarning::StartMoved {
            from: day(5),
            to: day(6)
        }));
        // "Does not repeat" on the same Monday keeps the first, Monday 05-10, on its own day: it
        // becomes one event there and nothing says it moves (C11d review 5).
        let once = EventPatchDraft {
            recurrence: Some(crate::calendar_write::dto::RepeatDraft::Once),
            ..monday.clone()
        };
        let stopped = case.edit(Scope::All, once).unwrap();
        assert_eq!(
            body(&stopped.steps[0])["start"],
            json!({ "dateTime": "2026-10-05T01:00:00-04:00", "timeZone": "America/New_York" })
        );
        assert_eq!(
            stopped.warnings,
            [SeriesWarning::BecomesOneEvent { day: day(5) }]
        );
        let rest = case.edit(Scope::Following, monday).unwrap();
        let new_part =
            json!({ "dateTime": "2026-10-19T01:00:00-04:00", "timeZone": "America/New_York" });
        let mondays = json!(["RRULE:FREQ=WEEKLY;BYDAY=MO"]);
        let starts_monday =
            |b: &Map<String, Value>| b["start"] == new_part && b["recurrence"] == mondays;
        assert!(rest
            .steps
            .iter()
            .any(|s| s.op.body().is_some_and(starts_monday)));
        let sunday = timed_in("2026-10-18", "20:00", "21:00", "America/New_York");
        for scope in [Scope::All, Scope::Following] {
            assert!(
                case.edit(scope, sunday.clone())
                    == Err(NotPlanned::Refused(SeriesRefusal::KindAndDay))
            );
        }
    }

    /// The series' own rule names its days, so it can't move them and change kind or zone in one save:
    /// refused in words, for every occurrence and from this one on alike.
    #[test]
    fn a_kind_or_zone_change_with_a_new_day_is_refused_in_words() {
        let tuesday_all_day = TimeDraft::AllDay {
            first_day: "2026-10-20".into(),
            last_day: "2026-10-20".into(),
        };
        let tuesday_in_new_york = TimeDraft::Timed {
            start_date: "2026-10-20".into(),
            start_time: "09:00".into(),
            start_zone: "America/New_York".into(),
            end_date: "2026-10-20".into(),
            end_time: "10:00".into(),
            end_zone: "America/New_York".into(),
        };
        let case = Case::new(weekly(), monday_19());
        for time in [tuesday_all_day, tuesday_in_new_york] {
            let draft = EventPatchDraft {
                time: Some(time),
                ..Default::default()
            };
            for scope in [Scope::All, Scope::Following] {
                let refused = case.edit(scope, draft.clone());
                assert!(refused == Err(NotPlanned::Refused(SeriesRefusal::KindAndDay)));
            }
        }
        // Shown on both rows, so worded for both (C11d review 1).
        assert_eq!(
            SeriesRefusal::KindAndDay.to_string(),
            "PM can't move these occurrences to another day and change them between all-day and \
             timed, or into another time zone, in one save. Change the day in one save and the \
             rest in another."
        );
        // The same day, another kind, goes.
        assert!(case
            .edit(
                Scope::All,
                EventPatchDraft {
                    time: Some(TimeDraft::AllDay {
                        first_day: "2026-10-19".into(),
                        last_day: "2026-10-19".into(),
                    }),
                    ..Default::default()
                }
            )
            .is_ok());
    }

    /// C11d review 1: an occurrence moved to Wednesday on its own and left there, made all-day or put
    /// in another zone, names no move the user made; moved further, it does; on its usual day, it
    /// goes.
    #[test]
    fn a_kind_or_zone_change_on_an_occurrence_moved_on_its_own_says_so() {
        let wednesday = moved(
            "2026-10-19T09:00:00+01:00",
            "2026-10-21T09:00:00+01:00",
            "2026-10-21T10:00:00+01:00",
        );
        let case = Case::new(weekly(), wednesday.clone()).exceptions(vec![wednesday]);
        let all_day_on = |date: &str| EventPatchDraft {
            time: Some(TimeDraft::AllDay {
                first_day: date.into(),
                last_day: date.into(),
            }),
            ..Default::default()
        };
        let in_new_york = timed_in("2026-10-21", "09:00", "10:00", "America/New_York");
        for scope in [Scope::All, Scope::Following] {
            for draft in [all_day_on("2026-10-21"), in_new_york.clone()] {
                assert!(
                    case.edit(scope, draft)
                        == Err(NotPlanned::Refused(SeriesRefusal::KindFromMovedDay))
                );
            }
            assert!(
                case.edit(scope, all_day_on("2026-10-22"))
                    == Err(NotPlanned::Refused(SeriesRefusal::KindAndDay))
            );
            assert!(case.edit(scope, all_day_on("2026-10-19")).is_ok());
        }
        assert_eq!(
            SeriesRefusal::KindFromMovedDay.to_string(),
            "This occurrence was moved to another day on its own, so PM can't change other \
             occurrences from it between all-day and timed, or into another time zone. Change it \
             on its own, or from an occurrence on its usual day."
        );
    }

    /// C11d review 1: weekly on Monday, ten times, from a Sunday its rule doesn't make (Google counts
    /// that Sunday). Rebuilt from its rule, the Sunday would go and an eleventh Monday would come, so
    /// a change of kind or zone for every occurrence is refused in words, from the Sunday as from any
    /// other: words of its own, naming no move, since the user moved nothing (C11d review 2). Its
    /// rest, from a Monday, still goes: the new part starts on an occurrence the rule makes.
    #[test]
    fn a_kind_change_on_all_refuses_a_series_starting_off_its_rule() {
        let from_sunday = |rule: &str| {
            master_with(
                &[rule],
                "2026-10-04T09:00:00+01:00",
                "2026-10-04T10:00:00+01:00",
            )
        };
        let all_day_on = |date: &str| EventPatchDraft {
            time: Some(TimeDraft::AllDay {
                first_day: date.into(),
                last_day: date.into(),
            }),
            ..Default::default()
        };
        let counted = from_sunday("RRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=10");
        let refused = Err(NotPlanned::Refused(SeriesRefusal::KindStartNotOnRule(day(
            2026, 10, 4,
        ))));
        let case = Case::new(counted.clone(), monday_19());
        assert!(case.edit(Scope::All, all_day_on("2026-10-19")) == refused);
        let in_new_york = timed_in("2026-10-19", "09:00", "10:00", "America/New_York");
        assert!(case.edit(Scope::All, in_new_york) == refused);
        assert_eq!(
            SeriesRefusal::KindStartNotOnRule(day(2026, 10, 4)).to_string(),
            "This repeating event starts on 04-10-2026, a day its own rule doesn't repeat on, so \
             PM can't change every occurrence between all-day and timed, or into another time \
             zone. Change this occurrence on its own."
        );
        let sunday = Case::new(counted, plain("2026-10-04T09:00:00+01:00"));
        for scope in [Scope::All, Scope::Following] {
            assert!(sunday.edit(scope, all_day_on("2026-10-04")) == refused);
        }
        let until = Case::new(
            from_sunday("RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261130T235959Z"),
            monday_19(),
        );
        assert!(until.edit(Scope::All, all_day_on("2026-10-19")) == refused);
        let rest = until
            .edit(Scope::Following, all_day_on("2026-10-19"))
            .unwrap();
        assert_eq!(
            body(&rest.steps[0])["start"],
            json!({ "date": "2026-10-19" })
        );
    }

    /// An all-day weekly series to 30-11, its 02-11 deleted by a dated EXDATE and its 26-10 cancelled,
    /// opened on 19-10.
    fn all_day_to_november() -> Case {
        let series = with(
            with(
                master(&[
                    "RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261130",
                    "EXDATE;VALUE=DATE:20261102",
                ]),
                "start",
                json!({ "date": "2026-10-05" }),
            ),
            "end",
            json!({ "date": "2026-10-06" }),
        );
        let on = |date: &str, status: &str| {
            json!({
                "kind": "calendar#event",
                "id": format!("abc_{}", date.replace('-', "")),
                "etag": "\"d1\"",
                "status": status,
                "summary": "Standup",
                "description": "Daily notes",
                "recurringEventId": "abc",
                "originalStartTime": { "date": date },
                "start": { "date": date },
                "end": { "date": "2026-10-20" },
                "organizer": { "email": "me@example.com", "self": true },
                "reminders": { "useDefault": true },
            })
        };
        Case::new(series, on("2026-10-19", "confirmed"))
            .exceptions(vec![on("2026-10-26", "cancelled")])
    }

    /// C11d review 1: from all-day to timed, every occurrence is rebuilt from the rule at the new
    /// time: dated keys cleared, the end date's UNTIL that day's last second in UTC, and the deleted
    /// days kept deleted as instants.
    #[test]
    fn a_kind_change_on_all_from_all_day_to_timed_rebuilds_from_the_series_rule() {
        let draft = at("09:00", "2026-10-19");
        let plan = all_day_to_november().edit(Scope::All, draft).unwrap();
        let sent = body(&plan.steps[0]);
        assert_eq!(
            sent["start"],
            json!({
                "dateTime": "2026-10-05T09:00:00+01:00",
                "timeZone": "Europe/London",
                "date": null
            })
        );
        assert_eq!(
            sent["end"],
            json!({
                "dateTime": "2026-10-05T10:00:00+01:00",
                "timeZone": "Europe/London",
                "date": null
            })
        );
        assert_eq!(
            sent["recurrence"],
            json!([
                "RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261130T235959Z",
                "EXDATE:20261026T090000Z,20261102T090000Z"
            ])
        );
        assert!(matches!(plan.rule_start, SeriesStart::Timed { .. }));
    }

    /// The same from this occurrence on: the new part timed from 19-10, its deleted days instants.
    #[test]
    fn a_kind_change_on_following_from_all_day_to_timed_rebuilds_the_new_part() {
        let draft = at("09:00", "2026-10-19");
        let plan = all_day_to_november().edit(Scope::Following, draft).unwrap();
        let insert = body(&plan.steps[0]);
        assert_eq!(
            insert["start"],
            json!({ "dateTime": "2026-10-19T09:00:00+01:00", "timeZone": "Europe/London" })
        );
        assert_eq!(
            insert["recurrence"],
            json!([
                "RRULE:FREQ=WEEKLY;BYDAY=MO;UNTIL=20261130T235959Z",
                "EXDATE:20261026T090000Z,20261102T090000Z"
            ])
        );
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
                None
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

    // --- C9a-2 ---

    /// DS12, C9 design review L3: a choice's token is what the plan does, bound to whom it was
    /// offered; never the etag it was planned on or the new series' id, which change every preview.
    #[test]
    fn a_choice_token_ignores_etags_and_tail_ids() {
        let split = |case: &Case, tail: &str| {
            plan_series_edit(Scope::Following, &case.input(), &title("Retro"), tail, None).unwrap()
        };
        let case = Case::new(weekly(), monday_19());
        let token = split(&case, TAIL).choice_digest("s1");
        assert_eq!(token.len(), 16);
        let other_tail = split(&case, "fedcba9876543210fedcba9876543210");
        assert_ne!(other_tail, split(&case, TAIL));
        assert_eq!(other_tail.choice_digest("s1"), token);
        let moved_on = Case::new(with(weekly(), "etag", json!("\"m2\"")), monday_19());
        assert_eq!(split(&moved_on, TAIL).choice_digest("s1"), token);
        // Offered to another editor, or another plan: another token.
        assert_ne!(split(&case, TAIL).choice_digest("s2"), token);
        let all = case.edit(Scope::All, title("Retro")).unwrap();
        assert_ne!(all.choice_digest("s1"), token);
        // One more occurrence changed on its own is one more carry: the user is asked again.
        let busier = Case::new(weekly(), monday_19()).exceptions(vec![with(
            plain("2026-10-26T09:00:00Z"),
            "summary",
            json!("Standup (room 4)"),
        )]);
        assert_ne!(split(&busier, TAIL).choice_digest("s1"), token);
    }

    /// C12: an occurrence with guests of its own no longer holds back what reaches it (C9a-2's
    /// refusal until Send / Don't send). A write to the whole series counts it as "at least", since
    /// PM can't tell whether Google emails that occurrence's guests too (probe L-R13-4); "this and
    /// following" past it still can't carry its guests onto the new part (Q3).
    #[test]
    fn an_occurrence_with_guests_of_its_own_is_counted_by_a_whole_series_write() {
        let invited = with(
            plain("2026-10-26T09:00:00Z"),
            "attendees",
            json!([{ "email": "a@example.com", "responseStatus": "needsAction" }]),
        );
        let cant_carry = Err(NotPlanned::Refused(SeriesRefusal::CannotCarry(day(
            2026, 10, 26,
        ))));
        let at_least = |plan: SeriesPlan| {
            plan.needs_notify()
                && plan
                    .guests
                    .is_some_and(|g| !g.complete && g.people == 0 && g.emails == 0)
        };
        let case = Case::new(weekly(), monday_19()).exceptions(vec![invited.clone()]);
        assert!(case.edit(Scope::This, title("Retro")).is_ok());
        assert!(case.delete(Scope::This).is_ok());
        assert_eq!(case.edit(Scope::Following, title("Retro")), cant_carry);
        assert!(at_least(case.edit(Scope::All, title("Retro")).unwrap()));
        assert!(at_least(case.delete(Scope::All).unwrap()));
        // A cut cancels that occurrence on the way: its guest is told, by count.
        let cut = case.delete(Scope::Following).unwrap();
        assert!(cut
            .guests
            .is_some_and(|g| g.cancelled && g.updated == 1 && g.complete));
        let after =
            Case::new(weekly(), plain("2026-11-02T09:00:00Z")).exceptions(vec![invited.clone()]);
        let split = after.edit(Scope::Following, title("Retro")).unwrap();
        assert!(split.guests.is_none(), "nobody past it to tell");
        assert!(after.delete(Scope::Following).is_ok());
        assert!(at_least(after.edit(Scope::All, title("Retro")).unwrap()));
        // From the first occurrence, "this and following" is every occurrence.
        let first =
            Case::new(weekly(), plain("2026-10-05T09:00:00+01:00")).exceptions(vec![invited]);
        assert!(at_least(first.delete(Scope::Following).unwrap()));
    }

    /// The over-correction named (C12 §7.2): C9a-2's guests rule refused a short listing for a change
    /// of text to every occurrence and for deleting them, since only a whole listing showed no
    /// occurrence had guests. With guests allowed those run, and say "some" may keep or lose their
    /// changes; the warning carries that, so the token tells "some" from a count. On a series with
    /// guests the count is "at least" too.
    #[test]
    fn a_short_listing_lets_text_go_to_all_events_and_says_some() {
        let mut short = Case::new(weekly(), monday_19());
        short.exceptions.complete = false;
        assert!(short.edit(Scope::This, title("Retro")).is_ok());
        assert!(short.delete(Scope::All).is_ok());
        let some = short.edit(Scope::All, title("Retro")).unwrap();
        assert_eq!(
            some.warnings,
            [SeriesWarning::ExceptionsMayReset {
                count: 0,
                complete: false
            }]
        );
        let seen = Case::new(weekly(), monday_19())
            .edit(Scope::All, title("Retro"))
            .unwrap();
        assert!(seen.warnings.is_empty());
        assert_ne!(some.choice_digest("s1"), seen.choice_digest("s1"));
        let mut guests = Case::new(with_guests(), monday_19());
        guests.exceptions.complete = false;
        let counted = guests.edit(Scope::All, title("Retro")).unwrap();
        assert!(counted
            .guests
            .is_some_and(|g| !g.complete && g.updated == 2 && g.people == 2));
        // The same count, seen whole or not: two tokens.
        let retitled = with(
            plain("2026-10-26T09:00:00Z"),
            "summary",
            json!("Standup (room 4)"),
        );
        let whole = Case::new(weekly(), monday_19()).exceptions(vec![retitled]);
        let mut part = Case::new(weekly(), monday_19());
        part.exceptions = Exceptions {
            items: whole.exceptions.items.clone(),
            complete: false,
        };
        let (whole, part) = (
            whole.edit(Scope::All, title("Retro")).unwrap(),
            part.edit(Scope::All, title("Retro")).unwrap(),
        );
        assert_eq!(
            (&whole.warnings[..], &part.warnings[..]),
            (
                &[SeriesWarning::ExceptionsMayReset {
                    count: 1,
                    complete: true
                }][..],
                &[SeriesWarning::ExceptionsMayReset {
                    count: 1,
                    complete: false
                }][..]
            )
        );
        assert_ne!(whole.choice_digest("s1"), part.choice_digest("s1"));
    }

    /// No guests rule refuses a short listing any more (C12), and the guards that need it whole
    /// still do: a cut, a split, and a move of every occurrence.
    #[test]
    fn a_short_listing_still_refuses_cuts_splits_and_moves() {
        let mut short = Case::new(weekly(), monday_19());
        short.exceptions.complete = false;
        let incomplete = Err(NotPlanned::Refused(SeriesRefusal::Incomplete));
        assert_eq!(short.delete(Scope::Following), incomplete);
        assert_eq!(short.edit(Scope::Following, title("Retro")), incomplete);
        assert_eq!(
            short.edit(Scope::All, at("09:00", "2026-10-20")),
            incomplete
        );
        // Deleting every occurrence needs none of them seen.
        assert!(short.delete(Scope::All).is_ok());
    }

    /// A29, C9 design review scope-L1: the rule and start a change leaves, for the question a move to
    /// another day asks; the master's rule where the change moves only the start.
    #[test]
    fn marks_from_keeps_the_originals_own_exceptions_from_the_split_on_by_content() {
        use chrono_tz::Europe::London;
        let at = point("2026-10-19T09:00:00+01:00");
        let before = with(
            plain("2026-10-12T09:00:00+01:00"),
            "summary",
            json!("Before"),
        );
        let after = with(plain("2026-11-02T09:00:00Z"), "summary", json!("Off-site"));
        let mut elsewhere = after.clone();
        elsewhere["recurringEventId"] = json!("other");
        let mut unplaced = plain("2026-11-09T09:00:00Z");
        unplaced["id"] = json!("abc_unplaced");
        unplaced
            .as_object_mut()
            .unwrap()
            .remove("originalStartTime");
        let items = vec![weekly(), before, after.clone(), elsewhere, unplaced];
        let marks = marks_from(&items, "abc", at, London);
        let ids: Vec<&str> = marks.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["abc_20261102T090000Z", "abc_unplaced"]);
        // Google touching it (a new etag, a new `updated`) is no change; a new title is.
        let touched = with(
            with(after.clone(), "etag", json!("\"z9\"")),
            "updated",
            json!("2026-10-18T00:00:00.000Z"),
        );
        assert_eq!(marks_from(&[touched], "abc", at, London), marks[..1]);
        let renamed = with(after, "summary", json!("Renamed"));
        assert_ne!(marks_from(&[renamed], "abc", at, London), marks[..1]);
    }

    #[test]
    fn a_new_part_is_read_against_what_its_insert_sent() {
        let plan = Case::new(weekly(), monday_19())
            .exceptions(vec![cancelled("2026-11-02T09:00:00Z")])
            .edit(Scope::Following, title("Retro"))
            .unwrap();
        let Op::Insert { body, .. } = &plan.steps[0].op else {
            panic!("not a split");
        };
        let mut copy = Value::Object(body.clone());
        copy["id"] = json!(TAIL);
        copy["etag"] = json!("\"t1\"");
        copy["status"] = json!("confirmed");
        assert!(plan.tail_as_inserted(0, &copy));
        // PM's own notes aside: a fence or a split note is bookkeeping.
        let mut noted = copy.clone();
        noted["extendedProperties"]["private"]["pmFence"] = json!("x");
        assert!(plan.tail_as_inserted(0, &noted));
        assert!(!plan.tail_as_inserted(0, &with(copy.clone(), "summary", json!("Renamed"))));
        assert!(!plan.tail_as_inserted(0, &with(copy.clone(), "status", json!("cancelled"))));
        // A field the original never had, added in Google since, or something an insert never
        // sends (review round 2).
        for (key, value) in [
            ("location", json!("Room 4")),
            ("colorId", json!("5")),
            ("attendees", json!([{ "email": "guest@example.com" }])),
            ("conferenceData", json!({ "conferenceId": "abc-defg-hij" })),
            ("hangoutLink", json!("https://meet.google.com/abc-defg-hij")),
            (
                "attachments",
                json!([{ "fileUrl": "https://drive.google.com/file/d/x" }]),
            ),
            ("eventLabelId", json!("label-1")),
        ] {
            assert!(
                !plan.tail_as_inserted(0, &with(copy.clone(), key, value)),
                "{key}"
            );
        }
        assert!(plan.tail_as_inserted(0, &with(copy.clone(), "attachments", Value::Null)));
        // Google's defaults, written out, are no change.
        let defaults = with(
            with(copy.clone(), "transparency", json!("opaque")),
            "visibility",
            json!("default"),
        );
        assert!(plan.tail_as_inserted(0, &defaults));
        assert!(!plan.tail_as_inserted(1, &copy));
        assert_eq!(plan.tail_exdates(0), [point("2026-11-02T09:00:00Z")]);
        assert!(plan.tail_exdates(1).is_empty());
    }

    #[test]
    fn a_change_says_the_rule_it_leaves() {
        let case = Case::new(weekly(), monday_19());
        let tuesdays = case.edit(Scope::All, at("09:00", "2026-10-20")).unwrap();
        let zone = chrono_tz::Europe::London;
        let at_nine = |y, m, d| SeriesStart::Timed {
            local: day(y, m, d).and_hms_opt(9, 0, 0).unwrap(),
            zone,
        };
        let weekly_rule = vec!["RRULE:FREQ=WEEKLY;BYDAY=MO".to_string()];
        assert_eq!(
            tuesdays.new_rule(&weekly_rule),
            Some((
                vec!["RRULE:FREQ=WEEKLY;BYDAY=TU".to_string()],
                at_nine(2026, 10, 6)
            ))
        );
        let rest = case
            .edit(Scope::Following, at("09:00", "2026-10-20"))
            .unwrap();
        assert_eq!(
            rest.new_rule(&weekly_rule),
            Some((
                vec!["RRULE:FREQ=WEEKLY;BYDAY=TU".to_string()],
                at_nine(2026, 10, 20)
            ))
        );
        // A rule that names no day keeps its text when its start moves.
        let monthly_rule = vec!["RRULE:FREQ=MONTHLY".to_string()];
        let monthly = master_with(
            &["RRULE:FREQ=MONTHLY"],
            "2026-10-05T09:00:00+01:00",
            "2026-10-05T10:00:00+01:00",
        );
        let sixth = Case::new(monthly, plain("2026-11-05T09:00:00Z"))
            .edit(Scope::All, at("09:00", "2026-11-06"))
            .unwrap();
        assert!(!body(&sixth.steps[0]).contains_key("recurrence"));
        assert_eq!(
            sixth.new_rule(&monthly_rule),
            Some((monthly_rule.clone(), at_nine(2026, 10, 6)))
        );
        // "This event", and a change of text only, leave the rule alone.
        assert_eq!(
            case.edit(Scope::This, at("09:00", "2026-10-20"))
                .unwrap()
                .new_rule(&weekly_rule),
            None
        );
        assert_eq!(
            case.edit(Scope::All, title("Retro"))
                .unwrap()
                .new_rule(&weekly_rule),
            None
        );
    }

    // --- creating an event (C10) ---

    const NEW: &str = "fedcba9876543210fedcba9876543210";

    /// What the editor sends for a new event, as the create command builds it.
    fn new_body() -> Map<String, Value> {
        super::super::super::patch::insert_body(&crate::calendar_write::dto::NewEventDraft {
            summary: "Dentist".into(),
            time: TimeDraft::Timed {
                start_date: "2026-10-26".into(),
                start_time: "10:00".into(),
                start_zone: "Europe/London".into(),
                end_date: "2026-10-26".into(),
                end_time: "11:00".into(),
                end_zone: "Europe/London".into(),
            },
            location: String::new(),
            description: String::new(),
            show_as: Some(ShowAs::Busy),
            visibility: None,
            recurrence: None,
        })
        .unwrap()
    }

    #[test]
    fn a_create_plan_is_one_commit_step_under_pms_id_with_no_one_notified() {
        let plan = SeriesPlan::create(CALENDAR, NEW, new_body()).unwrap();
        assert_eq!(plan.steps.len(), 1);
        let step = &plan.steps[0];
        assert!(matches!(&step.op, Op::Insert { calendar, id, body }
            if calendar == CALENDAR && id == NEW && *body == new_body()));
        assert_eq!(step.notify, Notify::None);
        assert_eq!(step.stage, Stage::Commit);
        assert!(step.requires.is_none() && step.undo.is_none() && !step.gone_is_done);
        assert_eq!(step.conflict, Recheck::Never);
        assert_eq!(step.shows, Shows::Created);
        assert!(step.op.fits());
        let request = step.op.request(step.notify).unwrap();
        assert_eq!(request.method, Method::Insert);
        assert!(request
            .url
            .ends_with("/events?sendUpdates=none&conferenceDataVersion=1"));
        assert_eq!(request.body.unwrap()["id"], NEW);
        assert!(plan.expect.is_empty() && plan.warnings.is_empty());
        assert_eq!(plan.removes, Removes::Nothing);
    }

    /// G5: a key a create may not send, a body with no start PM can read, or an id that isn't one
    /// path segment builds no plan at all. The repeat rule is one a create sends since C11.
    #[test]
    fn a_create_plan_refuses_a_key_a_create_may_not_send() {
        for key in [
            "attendees",
            "extendedProperties",
            "reminders",
            "conferenceData",
            "colorId",
            "id",
        ] {
            let mut body = new_body();
            body.insert(key.into(), json!([]));
            assert!(SeriesPlan::create(CALENDAR, NEW, body).is_none(), "{key}");
        }
        let mut no_start = new_body();
        no_start.insert("start".into(), json!({ "dateTime": "soon" }));
        assert!(SeriesPlan::create(CALENDAR, NEW, no_start).is_none());
        for id in ["", "..", "a\nb"] {
            assert!(
                SeriesPlan::create(CALENDAR, id, new_body()).is_none(),
                "{id:?}"
            );
        }
    }

    /// C11: a repeating event's body (its one rule line, its zone on both nodes) builds the same
    /// one-step plan, the rule in what it sends.
    #[test]
    fn a_create_plan_sends_a_repeating_events_rule() {
        let body =
            super::super::super::patch::insert_body(&crate::calendar_write::dto::NewEventDraft {
                recurrence: Some(crate::calendar_recur::wire::RepeatRule {
                    pattern: crate::calendar_recur::wire::RepeatPattern::Daily,
                    interval: 1,
                    ends: crate::calendar_recur::wire::RepeatEnds::After { count: 5 },
                }),
                summary: "Dentist".into(),
                time: TimeDraft::AllDay {
                    first_day: "2026-10-26".into(),
                    last_day: "2026-10-26".into(),
                },
                location: String::new(),
                description: String::new(),
                show_as: None,
                visibility: None,
            })
            .unwrap();
        let plan = SeriesPlan::create(CALENDAR, NEW, body).unwrap();
        let step = &plan.steps[0];
        assert!(step.op.fits());
        assert_eq!(step.notify, Notify::None);
        let request = step.op.request(step.notify).unwrap();
        let sent = request.body.unwrap();
        assert_eq!(sent["recurrence"], json!(["RRULE:FREQ=DAILY;COUNT=5"]));
        assert_eq!(sent["id"], NEW);
    }

    /// A live copy under PM's id is this create's, renamed or not; a cancelled one is this create's,
    /// deleted since; none proves nothing.
    #[test]
    fn created_reads_a_live_copy_as_landed_a_cancelled_one_as_not_and_none_as_unknown() {
        let plan = SeriesPlan::create(CALENDAR, NEW, new_body()).unwrap();
        let mut copy = Value::Object(new_body());
        copy["id"] = json!(NEW);
        copy["status"] = json!("confirmed");
        assert_eq!(plan.verdict(0, Some(&copy)), Applied::Yes);
        let renamed = with(copy.clone(), "summary", json!("Dentist (moved)"));
        assert_eq!(plan.verdict(0, Some(&renamed)), Applied::Yes);
        let cancelled = with(copy, "status", json!("cancelled"));
        assert_eq!(plan.verdict(0, Some(&cancelled)), Applied::No);
        assert_eq!(plan.verdict(0, None), Applied::Unknown);
    }

    // --- guests, and who a plan emails (C12a) ---

    use crate::calendar_write::dto::{GuestNotice, GuestOp, GuestState};

    const A: &str = "a@example.com";
    const B: &str = "b@example.com";

    /// A draft that sets these guests' states.
    fn guests_to(ops: &[(&str, GuestState)]) -> EventPatchDraft {
        EventPatchDraft {
            guests: Some(
                ops.iter()
                    .map(|(email, to)| GuestOp {
                        email: (*email).into(),
                        to: *to,
                    })
                    .collect(),
            ),
            ..Default::default()
        }
    }

    /// The weekly series and its Monday 19-10 occurrence, both with `list` as guests besides the
    /// user.
    fn invited(list: &[&str]) -> Case {
        Case::new(
            guests_on_it(weekly(), list),
            guests_on_it(monday_19(), list),
        )
    }

    /// Who each step reaches, as the plan was stamped by.
    fn reach_of_each(plan: &SeriesPlan, case: &Case) -> Vec<Option<GuestNotice>> {
        let input = case.input();
        let own = Own::of(
            &[CALENDAR, CALENDAR],
            &[case.master.json(), case.instance.json()],
        );
        plan.steps
            .iter()
            .map(|s| plan.reach_of(s, &input, &own).notice())
            .collect()
    }

    fn told(updated: u32, invited: u32, removed: u32, emails: u32) -> GuestNotice {
        GuestNotice {
            updated,
            invited,
            removed,
            told_cancelled: 0,
            cancelled: false,
            people: updated + invited + removed,
            emails,
            complete: true,
        }
    }

    /// C12 §6.3 (critique C1-M3: a delete, since a split past an occurrence with its own guests is
    /// refused): each step sends the answer only where it reaches someone; an occurrence whose guest
    /// was taken off on its own is cancelled with `none`.
    #[test]
    fn every_step_sends_the_answer_only_where_it_reaches_someone() {
        let alone = guests_on_it(
            with(plain("2026-11-02T09:00:00Z"), "summary", json!("Solo")),
            &[],
        );
        let shared = guests_on_it(
            with(plain("2026-11-09T09:00:00Z"), "summary", json!("Shared")),
            &[A],
        );
        let case = invited(&[A]).exceptions(vec![alone, shared]);
        let plan = case
            .delete_told(Scope::Following, Some(Notify::All))
            .unwrap();
        let kinds: Vec<(bool, Notify)> = plan
            .steps
            .iter()
            .map(|s| (matches!(s.op, Op::Patch { .. }), s.notify))
            .collect();
        assert!(
            kinds
                == [
                    (true, Notify::All),
                    (false, Notify::None),
                    (false, Notify::All)
                ],
            "the cut and the shared occurrence's cancel tell A; the solo one tells nobody"
        );
        assert!(
            plan.guests
                == Some(GuestNotice {
                    cancelled: true,
                    ..told(1, 0, 0, 2)
                }),
            "one guest, two emails, as a cancellation"
        );
        // Don't send: none everywhere, and it may run.
        let quiet = case
            .delete_told(Scope::Following, Some(Notify::None))
            .unwrap();
        assert!(quiet.steps.iter().all(|s| s.notify == Notify::None));
        assert!(!quiet.needs_notify());
        // No answer: placeholders, and it must not run.
        let unanswered = case.delete(Scope::Following).unwrap();
        assert!(unanswered.needs_notify());
        assert!(unanswered.steps.iter().all(|s| s.notify == Notify::None));
    }

    /// C12 critiques C2-H2 and C1-M2, re-derived: a "this and following" change of a series with A
    /// and B, one later occurrence moved on its own, adding c. Each step's reach is pinned: the new
    /// part invites three, the cut tells two, the carry onto the new part's occurrence tells its
    /// three, the old copy's cancel tells two. Ten emails, three people, and only c is new.
    #[test]
    fn this_and_following_counts_invitations_updates_and_occurrences() {
        let moved_one = guests_on_it(
            moved(
                "2026-11-02T09:00:00Z",
                "2026-11-03T09:00:00Z",
                "2026-11-03T10:00:00Z",
            ),
            &[A, B],
        );
        let case = invited(&[A, B]).exceptions(vec![moved_one]);
        let draft = guests_to(&[("c@example.com", GuestState::Required)]);
        let plan = case
            .edit_told(Scope::Following, draft, Some(Notify::All))
            .unwrap();
        let ops: Vec<&str> = plan
            .steps
            .iter()
            .map(|s| match s.op {
                Op::Insert { .. } => "insert",
                Op::Patch { .. } => "cut",
                Op::Carry { .. } => "carry",
                Op::Cancel { .. } => "cancel",
                Op::Delete { .. } => "delete",
            })
            .collect();
        assert_eq!(ops, ["insert", "cut", "carry", "cancel"]);
        assert!(
            reach_of_each(&plan, &case)
                == [
                    Some(told(0, 3, 0, 3)),
                    Some(told(2, 0, 0, 2)),
                    Some(told(0, 3, 0, 3)),
                    Some(GuestNotice {
                        cancelled: true,
                        ..told(2, 0, 0, 2)
                    }),
                ],
            "each step's own reach"
        );
        assert!(
            plan.guests
                == Some(GuestNotice {
                    updated: 2,
                    invited: 1,
                    removed: 0,
                    told_cancelled: 0,
                    cancelled: false,
                    people: 3,
                    emails: 10,
                    complete: true,
                }),
            "two already invited, one new, ten emails"
        );
        assert!(plan.steps.iter().all(|s| s.notify == Notify::All));
    }

    /// C12 critique C2-M7: a guest taken off from this one on is told they're off, not of an update.
    #[test]
    fn a_guest_left_off_the_new_part_is_counted_as_taken_off() {
        let case = invited(&[A, B]);
        let draft = guests_to(&[(B, GuestState::Absent)]);
        let plan = case
            .edit_told(Scope::Following, draft, Some(Notify::All))
            .unwrap();
        assert!(
            plan.guests == Some(told(1, 0, 1, 3)),
            "A updated, B taken off: the new part invites A, the cut tells both"
        );
    }

    /// C12 critiques C1-H1, C2-H1: the user is never a guest. A split of a series only the user is
    /// on emails nobody, and Google's copy of the new part, which marks the user `self`, still
    /// reads as what was inserted, so Remove isn't refused after a partial change.
    #[test]
    fn the_user_is_never_a_guest_of_the_new_part() {
        let case = Case::new(guests_on_it(weekly(), &[]), guests_on_it(monday_19(), &[]));
        let plan = case.edit(Scope::Following, title("Retro")).unwrap();
        assert!(plan.guests.is_none() && !plan.needs_notify());
        let sent = body(&plan.steps[0]).clone();
        assert_eq!(sent["attendees"], json!([{ "email": CALENDAR }]));
        let mut reply = Value::Object(sent);
        reply["id"] = json!(TAIL);
        reply["etag"] = json!("\"t1\"");
        reply["attendees"] = json!([
            { "email": CALENDAR, "self": true, "organizer": true, "responseStatus": "accepted" }
        ]);
        assert!(plan.tail_as_inserted(0, &reply));
        // A guest added to it in Google since is a change.
        reply["attendees"] = json!([
            { "email": CALENDAR, "self": true, "organizer": true },
            { "email": A }
        ]);
        assert!(!plan.tail_as_inserted(0, &reply));
    }

    /// C12 §6.4: the token binds who the plan emails, never their replies, and never the answer.
    #[test]
    fn the_token_changes_when_the_guests_do() {
        let digest = |case: &Case, notify: Option<Notify>| {
            case.edit_told(Scope::All, title("Retro"), notify)
                .unwrap()
                .choice_digest("s1")
        };
        let one = invited(&[A]);
        let two = invited(&[A, B]);
        assert!(digest(&one, None) != digest(&two, None));
        let mut replied = guests_on_it(weekly(), &[A]);
        replied["attendees"][1]["responseStatus"] = json!("declined");
        let mut one_replied = invited(&[A]);
        one_replied.master = fresh(replied.clone());
        one_replied.opened_master = fresh(replied);
        assert!(digest(&one, None) == digest(&one_replied, None));
        assert!(digest(&one, None) == digest(&one, Some(Notify::All)));
    }

    /// C12 §6.2: a write to every occurrence is "at least" where an occurrence has a guest list of
    /// its own, or where PM couldn't see every occurrence changed on its own; a short listing asks
    /// even with nobody seen (C12 critique C1, "short listings ask").
    #[test]
    fn a_whole_series_write_is_at_least_when_an_occurrence_has_guests_of_its_own() {
        let own_list = guests_on_it(
            with(plain("2026-10-26T09:00:00Z"), "summary", json!("Review")),
            &[A, B],
        );
        let case = invited(&[A]).exceptions(vec![own_list]);
        let plan = case
            .edit_told(Scope::All, title("Retro"), Some(Notify::All))
            .unwrap();
        assert!(
            plan.guests
                == Some(GuestNotice {
                    complete: false,
                    ..told(1, 0, 0, 1)
                }),
            "at least"
        );
        // Same guest lists everywhere: exact.
        let same = guests_on_it(
            with(plain("2026-10-26T09:00:00Z"), "summary", json!("Review")),
            &[A],
        );
        let plan = invited(&[A])
            .exceptions(vec![same])
            .edit(Scope::All, title("Retro"))
            .unwrap();
        assert!(plan.guests == Some(told(1, 0, 0, 1)), "exact");
        // A short listing, on a series with nobody else on it.
        let mut short = Case::new(weekly(), monday_19());
        short.exceptions.complete = false;
        let plan = short.edit(Scope::All, title("Retro")).unwrap();
        assert!(
            plan.guests
                == Some(GuestNotice {
                    complete: false,
                    ..GuestNotice::default()
                }),
            "asks, with nobody seen"
        );
        assert!(plan.needs_notify());
        // A personal change never asks, however short the listing.
        let busy = EventPatchDraft {
            show_as: Some(crate::calendar_write::dto::ShowAs::Free),
            ..Default::default()
        };
        assert!(short.edit(Scope::All, busy).unwrap().guests.is_none());
    }

    /// C12 §6.8: a guest change goes to the copy the scope names, as Google's own list with it.
    #[test]
    fn a_guest_change_to_this_event_patches_the_occurrence() {
        let case = invited(&[A]);
        let plan = case
            .edit(Scope::This, guests_to(&[(B, GuestState::Optional)]))
            .unwrap();
        let Op::Patch { event, body, .. } = &plan.steps[0].op else {
            panic!("not a patch");
        };
        assert_eq!(event, "abc_20261019T080000Z");
        let mut want = guests_on_it(monday_19(), &[A])["attendees"].clone();
        want.as_array_mut()
            .unwrap()
            .push(json!({ "email": B, "optional": true }));
        assert_eq!(body["attendees"], want);
        assert!(plan.guests == Some(told(1, 1, 0, 2)));
    }

    #[test]
    fn a_guest_change_to_all_events_patches_the_series() {
        let case = invited(&[A, B]);
        let plan = case
            .edit(Scope::All, guests_to(&[(A, GuestState::Absent)]))
            .unwrap();
        let Op::Patch { event, body, .. } = &plan.steps[0].op else {
            panic!("not a patch");
        };
        assert_eq!(event, "abc");
        assert_eq!(body["attendees"], guests_on_it(weekly(), &[B])["attendees"]);
        assert!(plan.guests == Some(told(1, 0, 1, 2)));
    }

    /// The new part takes the series' list with the change, each guest invited anew: no reply.
    #[test]
    fn a_guest_change_from_this_one_on_goes_to_the_new_part() {
        let case = invited(&[A]);
        let plan = case
            .edit(Scope::Following, guests_to(&[(B, GuestState::Required)]))
            .unwrap();
        assert_eq!(
            body(&plan.steps[0])["attendees"],
            json!([{ "email": CALENDAR }, { "email": A }, { "email": B }])
        );
        // The cut sends no guest list: the earlier part keeps its own.
        assert!(!body(&plan.steps[1]).contains_key("attendees"));
    }

    /// C12 §6.8: a guests-only change from this one on is a change like a title's, so it splits;
    /// one Google already holds is no change.
    #[test]
    fn a_guests_only_change_from_this_one_on_is_a_split() {
        let case = Case::new(weekly(), monday_19());
        let plan = case
            .edit(Scope::Following, guests_to(&[(A, GuestState::Required)]))
            .unwrap();
        assert!(matches!(plan.steps[0].op, Op::Insert { .. }));
        assert!(plan.guests == Some(told(0, 1, 0, 1)));
        let already = invited(&[A]);
        assert!(
            matches!(
                already.edit(Scope::Following, guests_to(&[(A, GuestState::Required)])),
                Err(NotPlanned::NoChange)
            ),
            "Google already has it"
        );
    }

    /// Whether `planned` was refused for `why` (never printed: a plan is Google's data).
    fn refused_for(planned: Result<SeriesPlan, NotPlanned>, why: SeriesRefusal) -> bool {
        matches!(planned, Err(NotPlanned::Refused(r)) if r == why)
    }

    /// C12 §6.7: flags Google leaves out at their defaults compare as Google means them, so a new
    /// part sent with an explicit default still reads as what was inserted.
    #[test]
    fn default_permissions_compare_as_google_means_them() {
        let probe = probe(&SeriesStart::AllDay(day(2026, 10, 19)));
        let (on, off) = (json!(true), json!(false));
        for (key, value, same) in [
            ("guestsCanInviteOthers", &on, true),
            ("guestsCanInviteOthers", &off, false),
            ("guestsCanSeeOtherGuests", &on, true),
            ("guestsCanModify", &off, true),
            ("guestsCanModify", &on, false),
            ("anyoneCanAddSelf", &off, true),
            ("privateCopy", &off, true),
            ("privateCopy", &on, false),
        ] {
            assert_eq!(probe.same(key, Some(value), None), same, "{key}");
        }
        let explicit = with(weekly(), "guestsCanInviteOthers", json!(true));
        let case = Case::new(explicit, monday_19());
        let plan = case.edit(Scope::Following, title("Retro")).unwrap();
        let mut reply = Value::Object(body(&plan.steps[0]).clone());
        if let Some(obj) = reply.as_object_mut() {
            obj.retain(|k, _| k != "guestsCanInviteOthers");
        }
        assert!(plan.tail_as_inserted(0, &reply));
    }

    /// C12 §6.5 and critiques C1-M6, C1-M5, C2-H3: a step planned to email nobody, with nobody's
    /// Don't send behind it, meets anyone Google would email as a conflict; the user and a room are
    /// no one to email. Under a Send, anyone Google's copy now adds to or takes off whom it emails is
    /// a conflict too, so the save asks again with the new numbers; a reply changes nobody.
    #[test]
    fn a_resend_asks_again_where_nobody_answered_for_who_hears() {
        let fresh_with = |list: Value| with(weekly(), "attendees", list);
        let guest = fresh_with(json!([{ "email": B }]));
        let room = fresh_with(
            json!([{ "email": "room1@resource.calendar.google.com", "resource": true }]),
        );
        let only_me = fresh_with(json!([{ "email": CALENDAR, "self": true }]));
        let alone = Case::new(weekly(), monday_19());
        let conflict = Resend::Conflict(vec!["attendees".into()]);
        for notify in [None, Some(Notify::All)] {
            let plan = alone.edit_told(Scope::All, title("Retro"), notify).unwrap();
            assert_eq!(plan.resend(0, &guest), conflict, "{notify:?}");
            assert_eq!(plan.resend(0, &room), Resend::Proceed);
            assert_eq!(plan.resend(0, &only_me), Resend::Proceed);
        }
        let quiet = alone
            .edit_told(Scope::All, title("Retro"), Some(Notify::None))
            .unwrap();
        assert_eq!(quiet.resend(0, &guest), Resend::Proceed);
        // A plan that tells A. Under Don't send, a guest who appeared is told nothing, as the rest.
        let with_a = invited(&[A]);
        let b_too = guests_on_it(weekly(), &[A, B]);
        let quiet = with_a
            .edit_told(Scope::All, title("Retro"), Some(Notify::None))
            .unwrap();
        assert_eq!(quiet.resend(0, &b_too), Resend::Proceed);
        // Under Send, the answer was for A alone.
        let sent = with_a
            .edit_told(Scope::All, title("Retro"), Some(Notify::All))
            .unwrap();
        assert_eq!(sent.resend(0, &b_too), conflict, "a guest who appeared");
        assert_eq!(
            sent.resend(0, &guests_on_it(weekly(), &[])),
            conflict,
            "a guest taken off"
        );
        let mut cut_short = guests_on_it(weekly(), &[A]);
        cut_short["attendeesOmitted"] = json!(true);
        assert_eq!(sent.resend(0, &cut_short), conflict, "a list cut short");
        let mut replied = guests_on_it(weekly(), &[A]);
        replied["attendees"][1]["responseStatus"] = json!("declined");
        assert_eq!(sent.resend(0, &replied), Resend::Proceed, "a reply");
        // Unanswered, it would never run; asked anyway, it's a conflict.
        let unanswered = with_a.edit(Scope::All, title("Retro")).unwrap();
        assert_eq!(unanswered.resend(0, &b_too), conflict);
    }

    /// C12 critiques C1-M5, C2-H3: a delete answered with Send stops at a 412 where Google's copy
    /// now tells anyone else, the whole series' and one occurrence's alike.
    #[test]
    fn a_delete_sent_again_tells_only_whom_the_user_answered_for() {
        let case = invited(&[A]);
        let conflict = Resend::Conflict(vec!["attendees".into()]);
        let whole = case.delete_told(Scope::All, Some(Notify::All)).unwrap();
        assert_eq!(whole.resend(0, &guests_on_it(weekly(), &[A, B])), conflict);
        assert_eq!(whole.resend(0, case.master.json()), Resend::Proceed);
        let one = case.delete_told(Scope::This, Some(Notify::All)).unwrap();
        assert_eq!(one.resend(0, &guests_on_it(monday_19(), &[A, B])), conflict);
        assert_eq!(one.resend(0, case.instance.json()), Resend::Proceed);
    }

    /// C12 decision 3: a guest list sent again after a 412 is Google's list then with the draft's
    /// guest changes, never the copy the plan read, so a reply, a comment or extra guests that landed
    /// meanwhile stay; and Google's list then has to be one they apply to, under Don't send too.
    #[test]
    fn a_guest_list_sent_again_is_built_on_googles_list_then() {
        let waiting = |event: Value| {
            let mut v = guests_on_it(event, &[A]);
            v["attendees"][1]["responseStatus"] = json!("needsAction");
            v
        };
        let case = Case::new(waiting(weekly()), waiting(monday_19()));
        let add_b = guests_to(&[(B, GuestState::Required)]);
        let conflict = Resend::Conflict(vec!["attendees".into()]);
        for (scope, now) in [(Scope::All, weekly()), (Scope::This, monday_19())] {
            let plan = case
                .edit_told(scope, add_b.clone(), Some(Notify::All))
                .unwrap();
            // A accepts in Google, with a comment and one more guest, before the write lands.
            let mut replied = guests_on_it(now.clone(), &[A]);
            replied["attendees"][1]["comment"] = json!("On my way");
            replied["attendees"][1]["additionalGuests"] = json!(1);
            assert_eq!(plan.resend(0, &replied), Resend::Proceed, "{scope:?}");
            let Some(Op::Patch { body, .. }) = plan.again_on(0, &replied) else {
                panic!("{scope:?}: no guest list sent again");
            };
            let mut want = replied["attendees"].clone();
            want.as_array_mut().unwrap().push(json!({ "email": B }));
            assert!(
                body["attendees"] == want,
                "{scope:?}: A's reply kept, B still added"
            );
            // Cut short since: the list can't be built on it.
            let quiet = case
                .edit_told(scope, add_b.clone(), Some(Notify::None))
                .unwrap();
            let mut cut_short = waiting(now);
            cut_short["attendeesOmitted"] = json!(true);
            assert_eq!(quiet.resend(0, &cut_short), conflict, "{scope:?}");
        }
        // A write with no guest list goes again as it was.
        let retitled = case
            .edit_told(Scope::All, title("Retro"), Some(Notify::All))
            .unwrap();
        assert!(retitled.again_on(0, case.master.json()).is_none());
    }

    /// C12 critiques C1-H1, C2-H1: the account's own address is the user where the list doesn't mark
    /// it, as on a calendar whose id isn't the account's: a change to the series emails nobody, nor
    /// does a new part, and the 412 rule doesn't count the user either.
    #[test]
    fn the_users_unmarked_address_is_never_a_guest() {
        const ME: &str = "bobby@example.org";
        let unmarked = |event: Value| {
            with(
                event,
                "attendees",
                json!([{ "email": ME, "responseStatus": "accepted" }]),
            )
        };
        let case = Case::new(unmarked(weekly()), unmarked(monday_19()));
        let input = SeriesInput {
            me: ME,
            ..case.input()
        };
        let all = plan_series_edit(Scope::All, &input, &title("Retro"), TAIL, None).unwrap();
        assert!(
            all.guests.is_none() && !all.needs_notify(),
            "a title change emails nobody"
        );
        assert_eq!(
            all.resend(0, case.master.json()),
            Resend::Proceed,
            "nor does the 412 rule count the user"
        );
        let split =
            plan_series_edit(Scope::Following, &input, &title("Retro"), TAIL, None).unwrap();
        assert!(matches!(split.steps[0].op, Op::Insert { .. }));
        assert!(split.guests.is_none(), "the new part invites nobody");
    }

    /// C12 §6.1 and critiques C2-M5, C1-M7, C1: guest changes are checked on the copy they go to;
    /// an occurrence with a list of its own can't send its changes to the others; what guests may do
    /// is the whole series'; a new part never copies a list Google cut short.
    #[test]
    fn guest_changes_are_refused_where_they_cant_go() {
        let case = invited(&[A]);
        for scope in [Scope::This, Scope::Following, Scope::All] {
            assert!(
                refused_for(
                    case.edit(scope, guests_to(&[(CALENDAR, GuestState::Absent)])),
                    SeriesRefusal::Guest(GuestError::CantRemove)
                ),
                "{scope:?}: the user stays"
            );
            assert!(
                refused_for(
                    case.edit(
                        scope,
                        guests_to(&[("not an address", GuestState::Required)])
                    ),
                    SeriesRefusal::Guest(GuestError::NotAnAddress)
                ),
                "{scope:?}: an address"
            );
        }
        // This occurrence had A taken off on its own.
        let mut own_list = invited(&[A]);
        own_list.instance = fresh(guests_on_it(monday_19(), &[]));
        own_list.opened_instance = own_list.instance.clone();
        let add_b = guests_to(&[(B, GuestState::Required)]);
        for scope in [Scope::Following, Scope::All] {
            assert!(
                refused_for(
                    own_list.edit(scope, add_b.clone()),
                    SeriesRefusal::GuestsDifferHere
                ),
                "{scope:?}: a list the others don't have"
            );
        }
        assert!(own_list.edit(Scope::This, add_b.clone()).is_ok());
        // What guests may do, never for one occurrence.
        let modify = EventPatchDraft {
            guests_can_modify: Some(true),
            ..Default::default()
        };
        assert!(refused_for(
            case.edit(Scope::This, modify.clone()),
            SeriesRefusal::GuestPermissionsAreSeriesWide
        ));
        assert!(case.edit(Scope::All, modify).is_ok());
        // A series whose list Google cut short: no new part copies part of it.
        let mut cut_short = guests_on_it(weekly(), &[A]);
        cut_short["attendeesOmitted"] = json!(true);
        let short = Case::new(cut_short, guests_on_it(monday_19(), &[A]));
        assert!(refused_for(
            short.edit(Scope::Following, title("Retro")),
            SeriesRefusal::Guest(GuestError::Omitted)
        ));
    }
}
