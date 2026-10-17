// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Running a [`SeriesPlan`] against Google without doing any I/O (C9a): a machine that asks for one
//! request at a time and is told each answer, so every reading of an answer is a table test with no
//! network, no clock and no database. The command layer, the only code that may send a calendar
//! write, drives it:
//!
//! ```text
//! loop {
//!     match machine.next() {
//!         Next::Send(request) => machine.answer(send(request)),
//!         Next::Wait(pause) => sleep(pause),
//!         Next::Over(result) => break result,
//!     }
//! }
//! ```
//!
//! [`Run`] sends a plan's steps in order by the rules in `plan.rs`'s module doc; [`Settle`] settles a
//! split the run couldn't finish ("Remove the new series", or look again). What the machine adds to
//! those rules, from the C9 design reviews (`docs/calendar-c9-design-2026-10-17.md`):
//!
//! - **Doubt is tracked per step.** A copy is in doubt when its answer was lost, met a server error
//!   (the sender reports one on any attempt, the last included, as `maybe_applied`, whatever the last
//!   attempt said), or was one classify can't read. Any answer but a 2xx to a step in doubt is
//!   checked with a GET first: a rate limit on the sender's retry says nothing about the copy before
//!   it (review H1).
//! - **"Once more on its original If-Match"** is the etag Google still shows, always one a copy went
//!   out on, and it is spent once per etag, counting the sender's own retry after a server error.
//!   Google showing what was there at an etag a copy went out on is never proof: proof that rests on a
//!   412 needs Google at an etag no copy went out on. While a copy may still land, a GET that shows
//!   neither the change nor what was there stays unknown (review M2).
//! - **A resend on Google's newer etag clears the doubt**: the copies before it went out on an older
//!   etag and can't land any more. At most four requests of a step leave the machine, two per etag on
//!   at most two etags, each on an etag PM has just seen; with the sender's own retry after a server
//!   error, up to six copies reach Google, and If-Match lets at most one per etag apply.
//! - **A lost insert** is looked for, looked for again after a second, and only then sent again under
//!   the same id, which Google refuses as a duplicate if the first copy landed (plan amendment A9).
//!   Its absence alone proves nothing.
//!
//! CodeQL guardrail (DECISIONS, 2026-10-10): nothing here uses `Vec::remove`, `insert` or
//! `swap_remove` on fetched data, or puts anything from a Google reply in a log or panic message.

use std::time::Duration;

use chrono::{DateTime, TimeDelta, Utc};
use serde_json::{json, Map, Value};

use crate::calendar_recur::lines::Applied;
use crate::calendar_recur::Point;

use super::super::classify::{Method, Verdict};
use super::super::dto::Notify;
use super::super::patch::is_gone;
use super::super::plan::{
    get_event, instances, is_plain_segment, patch_event, RequestPlan, VersionFlags,
};
use super::original_of;
use super::plan::{Op, Recheck, Resend, SeriesPlan, Shows, Stage, Undo};

/// Google's answer to one request the machine asked for, as the command layer read it.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "first made in C9a-3, from what Google answered")
)]
pub enum Answer {
    /// Google answered. `body`: the reply as JSON, when it is (a 2xx DELETE has none). `date`: its
    /// HTTP `Date`. `maybe_applied`: an attempt of this same request met a server error, the last or
    /// only one included, so a copy may have landed whatever this answer says.
    Reply {
        verdict: Verdict,
        body: Option<Value>,
        date: Option<DateTime<Utc>>,
        maybe_applied: bool,
    },
    /// No answer PM could read: offline, a timeout, a reply too large or unreadable, or a sign-in that
    /// failed after a copy met a server error. `retried`: the sender had already sent it once more
    /// after a server error.
    Lost { retried: bool },
}

/// What a machine wants done next.
#[derive(Debug, Clone, PartialEq)]
pub enum Next<T> {
    /// Send this, then tell the machine what came back ([`Machine::answer`]).
    Send(RequestPlan),
    /// Wait this long, then ask again.
    Wait(Duration),
    /// Finished.
    Over(T),
}

/// A machine the command layer drives: `next`, then `answer` after every `Send`.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "first caller lands in C9a-3, sending series writes"
    )
)]
pub trait Machine {
    type Output;
    fn next(&mut self) -> Next<Self::Output>;
    fn answer(&mut self, answer: Answer);
}

/// Where each step of a run ended.
#[derive(Debug, Clone, PartialEq)]
pub enum End {
    /// Not shown done: never sent, or the step the run stopped on. For [`Ran::Unknown`] that step may
    /// have landed; for [`Why::PutBack`], Google took half of it and that half was put back.
    NotRun,
    /// Landed. `reply`: Google's copy after it (the step's reply, or a GET that proved it), `None`
    /// for a delete. `date`: the HTTP `Date` of the answer that proved it, when it was the reply.
    Done {
        reply: Option<Value>,
        date: Option<DateTime<Utc>>,
    },
    /// A cleanup step not done or unknown, or one requiring such a step: left out, and reported
    /// ([`SeriesPlan::expect_after`] and [`SeriesPlan::removes_for`] take these).
    Skipped,
    /// Landed, then undone by a rollback.
    Undone,
}

/// Why a commit step provably didn't land.
#[derive(Debug, Clone, PartialEq)]
pub enum Why {
    /// Google answered 2xx and kept what was there: it ignored the change (the reported UNTIL fault,
    /// L-R5).
    Ignored,
    /// Changed in Google meanwhile, on these fields.
    Conflict(Vec<String>),
    /// The event is gone from Google.
    Gone,
    /// Refused before anything was applied.
    Refused(Verdict),
    /// Google took half of a move (the start without the rule, or the rule without the start), and
    /// the series' times and rule were put back. Only those: text sent with them may have landed.
    PutBack,
    /// A carry's occurrence never showed on the new series (cleanup steps only, so never in
    /// [`Ran::NotDone`]).
    NotFound,
}

/// What became of the steps before a commit step proved not done.
#[derive(Debug, Clone, PartialEq)]
pub enum Rollback {
    /// Nothing before it needed undoing.
    Nothing,
    /// Everything before it that needed undoing was undone.
    Done,
    /// Undoing step `step` couldn't be proved: what it made may still be in Google.
    Unknown { step: usize },
}

/// How a run ended.
#[derive(Debug, Clone, PartialEq)]
pub enum Ran {
    /// Every commit step landed; cleanup steps that didn't are [`End::Skipped`].
    Done { ends: Vec<End> },
    /// Commit step `step` provably isn't in Google (it didn't land, or its event is gone), and the
    /// steps before it were rolled back.
    NotDone {
        step: usize,
        why: Why,
        rollback: Rollback,
        ends: Vec<End>,
    },
    /// Commit step `step` may or may not have landed: the run stopped, and nothing was undone.
    /// `sent_on`: every If-Match a copy of it went out on (what proves it later, [`Settle`]).
    Unknown {
        step: usize,
        sent_on: Vec<String>,
        ends: Vec<End>,
    },
}

impl Ran {
    pub fn ends(&self) -> &[End] {
        match self {
            Ran::Done { ends } | Ran::NotDone { ends, .. } | Ran::Unknown { ends, .. } => ends,
        }
    }

    /// The steps left out, for [`SeriesPlan::expect_after`] and [`SeriesPlan::removes_for`].
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "first read in C9a-3, landing a series write")
    )]
    pub fn skipped(&self) -> Vec<usize> {
        self.ends()
            .iter()
            .enumerate()
            .filter(|(_, e)| **e == End::Skipped)
            .map(|(i, _)| i)
            .collect()
    }
}

/// A plan the machine won't send: a write holding a key its kind may not send (or a request it can't
/// build, should a kind ever have none), or a Finish without the proof it needs. A bug, never
/// something Google said (G5).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unfit;

/// The pause before a carry's second and third look for its occurrence (plan.rs: "after 1 s and
/// 3 s"), and before a lost insert's second look (A9).
const LOOK_AGAIN: [Duration; 2] = [Duration::from_secs(1), Duration::from_secs(3)];

/// One answer, read for the step it answers.
enum Read {
    Done(Option<Value>, Option<DateTime<Utc>>),
    NotDone(Why),
    Unknown,
    /// GET the step's event before deciding.
    Check,
    /// Send the same request once more on this etag (one a copy already went out on).
    Again(String),
    /// Send it on Google's newer etag: no copy of it can land any more.
    OnNewer(String),
    /// Put the series' times and rule back on this etag: Google took half the move.
    Restore(String),
    /// A lost insert's new series isn't there yet: look again after a pause.
    LookAgain,
}

/// What's been sent of the current step.
#[derive(Debug, Default)]
struct Copies {
    /// The step's own request as last sent.
    last: Option<RequestPlan>,
    /// Every If-Match a copy went out on.
    sent_on: Vec<String>,
    /// A copy may still land: its answer was lost, or it met a server error.
    in_doubt: bool,
    /// The etags the once-more has gone out on: once per etag, the sender's own retry after a server
    /// error counting as it.
    resent_on: Vec<String>,
    /// A lost insert was posted again under its id (A9), once.
    reposted: bool,
    /// The one resend on Google's newer etag after a 412 is spent.
    moved_on: bool,
    /// A copy was answered 2xx.
    answered_ok: bool,
    /// A carry's occurrence on the new series: its id.
    found: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// The step at `at` hasn't started.
    Start,
    /// A carry's look for its occurrence, this many so far, is out.
    Look(usize),
    /// Waiting before a carry's next look.
    LookPause(usize),
    /// A copy of the step's request is out.
    Sent,
    /// A GET of the step's event is out.
    Check,
    /// A lost insert's new series wasn't there: waiting before looking again.
    InsertPause,
    /// The look after that pause is out.
    InsertLook,
    /// The series' times and rule are being put back.
    Restore,
    /// The undo of the last step in `undo` is out (`resent`: it went once more already).
    UndoSend {
        resent: bool,
    },
    /// A GET of what that undo deletes is out.
    UndoCheck {
        resent: bool,
    },
    Over,
}

/// Runs one plan, step by step.
#[derive(Debug)]
pub struct Run {
    plan: SeriesPlan,
    ends: Vec<End>,
    at: usize,
    phase: Phase,
    copies: Copies,
    pending: Option<Answer>,
    /// The commit step proved not done, while the steps before it are undone.
    failed: Option<(usize, Why)>,
    /// Steps still to undo, the last one first.
    undo: Vec<usize>,
    result: Option<Ran>,
}

impl Run {
    /// Start a run of `plan`, or refuse it before anything is sent (G5).
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "first caller lands in C9a-3, sending series writes"
        )
    )]
    pub fn new(plan: SeriesPlan) -> Result<Run, Unfit> {
        let ends = vec![End::NotRun; plan.steps.len()];
        Run::from(plan, ends)
    }

    /// "Finish" the split `ran` left partial, on the proof [`Settle`] found: an unknown cut Google
    /// holds ([`Settled::CutDone`]) runs the cleanup after it; an unknown insert whose new series is
    /// there ([`Settled::TailThere`]) runs the cut onward. Anything else is [`Unfit`], so no cut is
    /// ever sent again on less than that (plan.rs: "unknown offers neither").
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "first caller lands in C9a-3, settling a partial split"
        )
    )]
    pub fn finish(plan: SeriesPlan, ran: &Ran, settled: &Settled) -> Result<Run, Unfit> {
        let insert = plan
            .steps
            .iter()
            .position(|s| matches!(s.op, Op::Insert { .. }))
            .ok_or(Unfit)?;
        let cut = insert + 1;
        let Ran::Unknown { step, ends, .. } = ran else {
            return Err(Unfit);
        };
        if ends.len() != plan.steps.len() {
            return Err(Unfit);
        }
        let mut ends = ends.clone();
        let proved = |copy: &Value| End::Done {
            reply: Some(copy.clone()),
            date: None,
        };
        // The copy has to show what it's offered as, by this plan's own reading of that step.
        let shows = |step: usize, copy: &Value| plan.verdict(step, Some(copy)) == Applied::Yes;
        match settled {
            Settled::CutDone { original } if *step == cut && shows(cut, original) => {
                ends[cut] = proved(original);
            }
            Settled::TailThere { tail } if *step == insert && shows(insert, tail) => {
                ends[insert] = proved(tail);
            }
            _ => return Err(Unfit),
        }
        Run::from(plan, ends)
    }

    fn from(plan: SeriesPlan, ends: Vec<End>) -> Result<Run, Unfit> {
        let fits = plan.steps.iter().all(|s| {
            s.op.fits() && (matches!(s.op, Op::Carry { .. }) || s.op.request(s.notify).is_some())
        });
        if !fits || ends.len() != plan.steps.len() || ends.contains(&End::Undone) {
            return Err(Unfit);
        }
        let at = ends
            .iter()
            .position(|e| *e == End::NotRun)
            .unwrap_or(ends.len());
        if ends[at..].iter().any(|e| *e != End::NotRun) {
            return Err(Unfit);
        }
        // Every commit step before where it starts landed, and a new series it may have to undo has
        // Google's copy to undo it on: never a cut after an insert that isn't proved.
        let landed = plan.steps[..at].iter().zip(&ends).all(|(s, e)| match e {
            End::Done { reply, .. } => match s.undo {
                Some(Undo::DeleteInserted { .. }) => reply.as_ref().and_then(etag_of).is_some(),
                _ => true,
            },
            _ => s.stage == Stage::Cleanup,
        });
        if !landed {
            return Err(Unfit);
        }
        Ok(Run {
            plan,
            ends,
            at,
            phase: Phase::Start,
            copies: Copies::default(),
            pending: None,
            failed: None,
            undo: Vec::new(),
            result: None,
        })
    }

    /// The plan being run, for landing its result ([`SeriesPlan::expect_after`]).
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "first read in C9a-3, landing a series write")
    )]
    pub fn plan(&self) -> &SeriesPlan {
        &self.plan
    }

    fn advance(&mut self) -> Next<Ran> {
        // Each pass handles one phase; a phase that needs no request moves on to the next.
        loop {
            if let Some(result) = &self.result {
                return Next::Over(result.clone());
            }
            let answer = self.pending.take();
            let next = match (self.phase, answer) {
                (Phase::Start, None) => self.start(),
                (Phase::Look(n), Some(a)) => self.on_look(n, a),
                (Phase::LookPause(n), None) => Some(self.look(n + 1)),
                (Phase::Sent, Some(a)) => {
                    let read = self.read_write(a);
                    self.apply(read)
                }
                (Phase::Check, Some(a)) => {
                    let read = self.read_check(a);
                    self.apply(read)
                }
                (Phase::InsertPause, None) => {
                    self.phase = Phase::InsertLook;
                    Some(Next::Send(self.check_request()))
                }
                (Phase::InsertLook, Some(a)) => self.on_insert_look(a),
                (Phase::Restore, Some(a)) => {
                    let read = match a {
                        Answer::Reply {
                            verdict: Verdict::Ok,
                            body: Some(b),
                            ..
                        } if self.plan.restored(self.at, &b) => Read::NotDone(Why::PutBack),
                        _ => Read::Unknown,
                    };
                    self.apply(read)
                }
                (Phase::UndoSend { resent }, Some(a)) => self.on_undo_send(resent, a),
                (Phase::UndoCheck { resent }, Some(a)) => self.on_undo_check(resent, a),
                // Asked for the next request while one is still out, or told an answer nothing asked
                // for: the driver broke the protocol. Stop rather than guess what was sent.
                _ => Some(self.stop_unknown()),
            };
            if let Some(next) = next {
                return next;
            }
        }
    }

    fn start(&mut self) -> Option<Next<Ran>> {
        let Some(step) = self.plan.steps.get(self.at) else {
            return Some(self.over(Ran::Done {
                ends: self.ends.clone(),
            }));
        };
        if let Some(r) = step.requires {
            if !matches!(self.ends.get(r), Some(End::Done { .. })) {
                self.ends[self.at] = End::Skipped;
                self.at += 1;
                return None;
            }
        }
        self.copies = Copies::default();
        if matches!(step.op, Op::Carry { .. }) {
            return Some(self.look(1));
        }
        match step.op.request(step.notify) {
            Some(request) => Some(self.send(request)),
            None => Some(self.stop_unknown()),
        }
    }

    /// Send a copy of the step's request.
    fn send(&mut self, request: RequestPlan) -> Next<Ran> {
        if let Some(etag) = &request.if_match {
            if !self.copies.sent_on.contains(etag) {
                self.copies.sent_on.push(etag.clone());
            }
        }
        self.copies.last = Some(request.clone());
        self.phase = Phase::Sent;
        Next::Send(request)
    }

    /// A carry's `n`th look for its occurrence on the new series.
    fn look(&mut self, n: usize) -> Next<Ran> {
        let Some(Op::Carry {
            calendar,
            series,
            original,
            ..
        }) = self.plan.steps.get(self.at).map(|s| &s.op)
        else {
            return self.stop_unknown();
        };
        let request = carry_lookup(calendar, series, *original);
        self.phase = Phase::Look(n);
        Next::Send(request)
    }

    fn on_look(&mut self, n: usize, answer: Answer) -> Option<Next<Ran>> {
        let step = &self.plan.steps[self.at];
        let Op::Carry { original, .. } = &step.op else {
            return Some(self.stop_unknown());
        };
        let found = match answer {
            Answer::Reply {
                verdict: Verdict::Ok,
                body: Some(list),
                ..
            } => find_occurrence(&list, *original),
            _ => None,
        };
        match found {
            Some((id, etag)) => match step.op.carry_request(&id, &etag, step.notify) {
                Some(request) => {
                    self.copies.found = Some(id);
                    Some(self.send(request))
                }
                None => self.apply(Read::NotDone(Why::NotFound)),
            },
            None => match LOOK_AGAIN.get(n - 1) {
                Some(pause) => {
                    self.phase = Phase::LookPause(n);
                    Some(Next::Wait(*pause))
                }
                None => self.apply(Read::NotDone(Why::NotFound)),
            },
        }
    }

    /// Read the answer to a copy of the step's own request.
    fn read_write(&mut self, answer: Answer) -> Read {
        let method = self.method();
        let (verdict, body, date) = match answer {
            Answer::Lost { retried } => {
                self.copies.in_doubt = true;
                if retried {
                    self.once_more_spent();
                }
                return Read::Check;
            }
            Answer::Reply {
                verdict,
                body,
                date,
                maybe_applied,
            } => {
                if maybe_applied {
                    self.copies.in_doubt = true;
                    // The sender re-sends a PATCH or DELETE once after a server error, on the same
                    // If-Match: that was the once-more, unless it was refused before Google read it.
                    // (A rate limit, then a server error, also reads as spent: the safe way round.)
                    if matches!(method, Method::Patch | Method::Delete) && !refused(&verdict) {
                        self.once_more_spent();
                    }
                }
                (verdict, body, date)
            }
        };
        // A server error may have been applied whatever the flag says, and an answer classify can't
        // read (a 409 to a change is no insert's duplicate) proves nothing either way.
        if matches!(verdict, Verdict::ServerError | Verdict::Other(..))
            || (verdict == Verdict::Duplicate && method != Method::Insert)
        {
            self.copies.in_doubt = true;
        }
        if verdict == Verdict::Ok {
            self.copies.answered_ok = true;
            if method == Method::Delete {
                return Read::Done(None, date);
            }
            return match body {
                Some(b) => match self.plan.verdict(self.at, Some(&b)) {
                    Applied::Yes => Read::Done(Some(b), date),
                    // This copy was applied, on the etag it went out on, and Google kept what was
                    // there: it ignored the change. No earlier copy can land after it.
                    Applied::No => Read::NotDone(Why::Ignored),
                    Applied::Unknown => Read::Check,
                },
                None => Read::Check,
            };
        }
        if self.copies.in_doubt {
            return Read::Check;
        }
        match verdict {
            Verdict::Gone => match self.plan.verdict(self.at, None) {
                Applied::Yes => Read::Done(None, date),
                Applied::No => Read::NotDone(Why::Gone),
                // A new series not found proves nothing (DS5): look for it.
                Applied::Unknown => Read::Check,
            },
            v if refused(&v) => Read::NotDone(Why::Refused(v)),
            // A 412 (a GET before any 412 rule), a 409 on an insert (a GET of the id), and anything
            // classify doesn't call final.
            _ => Read::Check,
        }
    }

    /// Read the GET of the step's event after an answer that didn't settle it.
    fn read_check(&mut self, answer: Answer) -> Read {
        let fresh = match answer {
            Answer::Reply {
                verdict: Verdict::Ok,
                body: Some(b),
                ..
            } => Some(b),
            Answer::Reply {
                verdict: Verdict::Gone,
                ..
            } => None,
            _ => return Read::Unknown,
        };
        match self.plan.verdict(self.at, fresh.as_ref()) {
            Applied::Yes => Read::Done(fresh, None),
            Applied::No => self.not_landed(fresh),
            Applied::Unknown => match &fresh {
                // Someone else changed what the step writes, and no copy of it can land: the 412
                // rule's own proof (plan.rs: "a 412 whose resend is a conflict"). Before the
                // half-move check: a half move that no copy of PM's made is someone else's, never
                // PM's to put back.
                Some(f) if self.none_can_land(f) => match self.plan.resend(self.at, f) {
                    Resend::Conflict(fields) => Read::NotDone(Why::Conflict(fields)),
                    Resend::Gone if self.step_gone_is_done() => Read::Done(None, None),
                    Resend::Gone => Read::NotDone(Why::Gone),
                    Resend::Proceed => Read::Unknown,
                },
                Some(f) if self.plan.half_applied(self.at, f) => match etag_of(f) {
                    Some(etag) => Read::Restore(etag),
                    None => Read::Unknown,
                },
                None if self.method() == Method::Insert && !self.copies.reposted => Read::LookAgain,
                _ => Read::Unknown,
            },
        }
    }

    /// The once-more on the etag the last copy went out on is spent (an insert's, under its id).
    fn once_more_spent(&mut self) {
        match self.copies.last.as_ref().and_then(|r| r.if_match.clone()) {
            Some(etag) => self.copies.resent_on.push(etag),
            None => self.copies.reposted = true,
        }
    }

    /// Whether no copy of the step can land with Google at `fresh`: every copy was answered and
    /// refused (none in doubt, none applied), and Google is at an etag none went out on. Never for an
    /// insert or a carry, which have no etag of their own to be refused on.
    fn none_can_land(&self, fresh: &Value) -> bool {
        matches!(self.method(), Method::Patch | Method::Delete)
            && !matches!(self.plan.steps[self.at].conflict, Recheck::Never)
            && !self.copies.in_doubt
            && !self.copies.answered_ok
            && etag_of(fresh).is_some_and(|e| !self.copies.sent_on.contains(&e))
    }

    /// Google shows the step not landed (`None`: the event is gone). Proof, or a reason to send again.
    fn not_landed(&mut self, fresh: Option<Value>) -> Read {
        // Gone, the event can't hold what was sent, wherever any copy is.
        let Some(fresh) = fresh else {
            return Read::NotDone(Why::Gone);
        };
        // A copy was applied, yet Google shows what was there: changed back since? Don't guess.
        if self.copies.answered_ok {
            return Read::Unknown;
        }
        let Some(etag) = etag_of(&fresh) else {
            return Read::Unknown;
        };
        if self.copies.sent_on.contains(&etag) {
            // At an etag a copy went out on, what was there proves nothing: a copy still on its way
            // can change it, and a 412 the GET contradicts is no reading at all. Send the same request
            // once more on it; a late first copy makes that a 412, and the next GET shows it landed.
            return if self.copies.resent_on.contains(&etag) {
                Read::Unknown
            } else {
                Read::Again(etag)
            };
        }
        if self.method() == Method::Insert {
            // An insert has no etag, so nothing above applies; a step that shows what was there
            // can't be an insert (its verdict is never No).
            return Read::Unknown;
        }
        // Google is at an etag no copy went out on: none of them can land.
        match self.plan.resend(self.at, &fresh) {
            Resend::Proceed if self.copies.moved_on => Read::NotDone(Why::Conflict(Vec::new())),
            Resend::Proceed => Read::OnNewer(etag),
            Resend::Gone if self.step_gone_is_done() => Read::Done(None, None),
            Resend::Gone => Read::NotDone(Why::Gone),
            Resend::Conflict(fields) => Read::NotDone(Why::Conflict(fields)),
        }
    }

    fn on_insert_look(&mut self, answer: Answer) -> Option<Next<Ran>> {
        match answer {
            Answer::Reply {
                verdict: Verdict::Ok,
                body: Some(b),
                ..
            } if self.plan.verdict(self.at, Some(&b)) == Applied::Yes => {
                self.apply(Read::Done(Some(b), None))
            }
            // Still not there after a pause: post it again under the same id (A9). If the first
            // copy landed after all, Google refuses this one as a duplicate.
            Answer::Reply {
                verdict: Verdict::Gone,
                ..
            } => match self.copies.last.clone() {
                Some(request) => {
                    self.copies.reposted = true;
                    Some(self.send(request))
                }
                None => self.apply(Read::Unknown),
            },
            _ => self.apply(Read::Unknown),
        }
    }

    fn apply(&mut self, read: Read) -> Option<Next<Ran>> {
        match read {
            Read::Check => {
                self.phase = Phase::Check;
                Some(Next::Send(self.check_request()))
            }
            Read::Again(etag) => {
                let Some(request) = self.copies.last.clone().map(|r| on_etag(r, &etag)) else {
                    return Some(self.stop_unknown());
                };
                self.copies.resent_on.push(etag);
                Some(self.send(request))
            }
            Read::OnNewer(etag) => {
                let Some(request) = self.copies.last.clone().map(|r| on_etag(r, &etag)) else {
                    return Some(self.stop_unknown());
                };
                self.copies.in_doubt = false;
                self.copies.answered_ok = false;
                self.copies.moved_on = true;
                Some(self.send(request))
            }
            Read::Restore(etag) => {
                let step = &self.plan.steps[self.at];
                let Some(request) = step.undo.as_ref().map(|u| u.request(&etag, step.notify))
                else {
                    return Some(self.stop_unknown());
                };
                self.phase = Phase::Restore;
                Some(Next::Send(request))
            }
            Read::LookAgain => {
                self.phase = Phase::InsertPause;
                Some(Next::Wait(LOOK_AGAIN[0]))
            }
            Read::Done(reply, date) => {
                self.ends[self.at] = End::Done { reply, date };
                self.at += 1;
                self.phase = Phase::Start;
                None
            }
            Read::NotDone(_) | Read::Unknown
                if self.plan.steps[self.at].stage == Stage::Cleanup =>
            {
                self.ends[self.at] = End::Skipped;
                self.at += 1;
                self.phase = Phase::Start;
                None
            }
            Read::NotDone(why) => {
                self.failed = Some((self.at, why));
                self.undo = (0..self.at)
                    .filter(|j| matches!(self.ends[*j], End::Done { .. }))
                    .filter(|j| {
                        matches!(self.plan.steps[*j].undo, Some(Undo::DeleteInserted { .. }))
                    })
                    .collect();
                self.next_undo()
            }
            Read::Unknown => Some(self.stop_unknown()),
        }
    }

    /// Undo the last step left in `undo`, or report the rollback once none is left.
    fn next_undo(&mut self) -> Option<Next<Ran>> {
        let Some(&j) = self.undo.last() else {
            let Some((step, why)) = self.failed.take() else {
                return Some(self.stop_unknown());
            };
            let rollback = if self.ends.contains(&End::Undone) {
                Rollback::Done
            } else {
                Rollback::Nothing
            };
            return Some(self.over(Ran::NotDone {
                step,
                why,
                rollback,
                ends: self.ends.clone(),
            }));
        };
        let etag = match &self.ends[j] {
            End::Done { reply: Some(r), .. } => etag_of(r),
            _ => None,
        };
        let Some(undo) = self.plan.steps[j].undo.as_ref() else {
            return Some(self.rollback_unknown(j));
        };
        match etag {
            Some(etag) => {
                self.phase = Phase::UndoSend { resent: false };
                Some(Next::Send(undo.request(&etag, self.plan.steps[j].notify)))
            }
            None => {
                self.phase = Phase::UndoCheck { resent: true };
                Some(Next::Send(undo_target(undo)))
            }
        }
    }

    /// The step being undone, and its undo.
    fn undoing(&self) -> Option<(usize, Undo)> {
        let j = *self.undo.last()?;
        Some((j, self.plan.steps[j].undo.clone()?))
    }

    fn on_undo_send(&mut self, resent: bool, answer: Answer) -> Option<Next<Ran>> {
        let Some((j, undo)) = self.undoing() else {
            return Some(self.stop_unknown());
        };
        match answer {
            Answer::Reply {
                verdict: Verdict::Ok,
                ..
            } => self.undone(j),
            Answer::Reply {
                verdict: Verdict::Gone,
                ..
            } if undo.gone_is_done() => self.undone(j),
            Answer::Reply {
                verdict,
                maybe_applied: false,
                ..
            } if refused(&verdict) => Some(self.rollback_unknown(j)),
            // A 412, a server error, a lost answer: look before sending again.
            _ => {
                self.phase = Phase::UndoCheck { resent };
                Some(Next::Send(undo_target(&undo)))
            }
        }
    }

    fn on_undo_check(&mut self, resent: bool, answer: Answer) -> Option<Next<Ran>> {
        let Some((j, undo)) = self.undoing() else {
            return Some(self.stop_unknown());
        };
        match answer {
            Answer::Reply {
                verdict: Verdict::Gone,
                ..
            } if undo.gone_is_done() => self.undone(j),
            Answer::Reply {
                verdict: Verdict::Ok,
                body: Some(fresh),
                ..
            } => {
                if is_gone(&fresh) && undo.gone_is_done() {
                    return self.undone(j);
                }
                match etag_of(&fresh) {
                    Some(etag) if !resent && undo.resend_on(&fresh) => {
                        self.phase = Phase::UndoSend { resent: true };
                        Some(Next::Send(undo.request(&etag, self.plan.steps[j].notify)))
                    }
                    _ => Some(self.rollback_unknown(j)),
                }
            }
            _ => Some(self.rollback_unknown(j)),
        }
    }

    fn undone(&mut self, j: usize) -> Option<Next<Ran>> {
        self.ends[j] = End::Undone;
        self.undo.pop();
        self.next_undo()
    }

    fn rollback_unknown(&mut self, j: usize) -> Next<Ran> {
        let (step, why) = self
            .failed
            .take()
            .unwrap_or((self.at, Why::Conflict(Vec::new())));
        self.over(Ran::NotDone {
            step,
            why,
            rollback: Rollback::Unknown { step: j },
            ends: self.ends.clone(),
        })
    }

    fn stop_unknown(&mut self) -> Next<Ran> {
        self.over(Ran::Unknown {
            step: self.at,
            sent_on: self.copies.sent_on.clone(),
            ends: self.ends.clone(),
        })
    }

    fn over(&mut self, result: Ran) -> Next<Ran> {
        self.phase = Phase::Over;
        self.result = Some(result.clone());
        Next::Over(result)
    }

    fn method(&self) -> Method {
        match &self.plan.steps[self.at].op {
            Op::Insert { .. } => Method::Insert,
            Op::Delete { .. } => Method::Delete,
            Op::Patch { .. } | Op::Cancel { .. } | Op::Carry { .. } => Method::Patch,
        }
    }

    fn step_gone_is_done(&self) -> bool {
        self.plan.steps[self.at].gone_is_done
    }

    /// A GET of what the current step writes.
    fn check_request(&self) -> RequestPlan {
        match &self.plan.steps[self.at].op {
            Op::Insert { calendar, id, .. } => get_event(calendar, id),
            Op::Patch {
                calendar, event, ..
            }
            | Op::Cancel {
                calendar, event, ..
            }
            | Op::Delete {
                calendar, event, ..
            } => get_event(calendar, event),
            Op::Carry { calendar, .. } => {
                get_event(calendar, self.copies.found.as_deref().unwrap_or_default())
            }
        }
    }
}

impl Machine for Run {
    type Output = Ran;

    fn next(&mut self) -> Next<Ran> {
        self.advance()
    }

    fn answer(&mut self, answer: Answer) {
        self.pending = Some(answer);
    }
}

/// Whether classify calls `verdict` final before anything is applied (plan.rs's list).
fn refused(verdict: &Verdict) -> bool {
    matches!(
        verdict,
        Verdict::BadRequest(_)
            | Verdict::ReadOnly
            | Verdict::NotOrganizer
            | Verdict::InsufficientScope
            | Verdict::QuotaExceeded
            | Verdict::Reauth
            | Verdict::RateLimited
    )
}

fn etag_of(event: &Value) -> Option<String> {
    event
        .get("etag")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// `request`, sent on `etag` (an insert has none, and stays as it is).
fn on_etag(mut request: RequestPlan, etag: &str) -> RequestPlan {
    if request.if_match.is_some() {
        request.if_match = Some(etag.to_string());
    }
    request
}

/// A GET of what `undo` writes.
fn undo_target(undo: &Undo) -> RequestPlan {
    match undo {
        Undo::DeleteInserted {
            calendar, event, ..
        }
        | Undo::RestoreMaster {
            calendar, event, ..
        } => get_event(calendar, event),
    }
}

/// The occurrences of `series` within a day either side of `original`, where a carry's occurrence
/// is: instants for an all-day series too, which Google's `timeMin`/`timeMax` require (review L5).
fn carry_lookup(calendar: &str, series: &str, original: Point) -> RequestPlan {
    let (from, to) = match original {
        Point::At(t) => (t - TimeDelta::days(1), t + TimeDelta::days(1)),
        Point::Day(d) => {
            let midnight = d.and_time(chrono::NaiveTime::MIN).and_utc();
            (midnight - TimeDelta::days(1), midnight + TimeDelta::days(2))
        }
    };
    let rfc = |t: DateTime<Utc>| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    instances(
        calendar,
        series,
        &[
            ("timeMin", &rfc(from)),
            ("timeMax", &rfc(to)),
            ("maxResults", "10"),
        ],
    )
}

/// The id and etag of the occurrence whose `originalStartTime` is `original` in a listing: by where it
/// sits in its series, never by its id (ids are opaque), and never a cancelled one (the listing shows
/// those too).
fn find_occurrence(list: &Value, original: Point) -> Option<(String, String)> {
    list.get("items")?.as_array()?.iter().find_map(|item| {
        if is_gone(item) || original_of(item) != Some(original) {
            return None;
        }
        let id = item.get("id")?.as_str().filter(|id| is_plain_segment(id))?;
        Some((id.to_string(), etag_of(item)?))
    })
}

/// What to do about a split a run left partial.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(
    not(test),
    expect(dead_code, reason = "first chosen in C9a-3, settling a partial split")
)]
pub enum SettleAction {
    /// Look again: GETs only.
    Check,
    /// "Remove the new series": delete it, once the original is proved not cut.
    Remove,
}

/// Where a partial split stands.
#[derive(Debug, Clone, PartialEq)]
pub enum Settled {
    /// Google holds the cut: "Finish" runs the rest of the plan ([`Run::finish`] takes this copy of
    /// the original as the cut's proof). Remove refuses here: it would delete the only copy of every
    /// occurrence from the split on.
    CutDone { original: Value },
    /// The new series is there and the original was never cut: Finish sends the cut onward
    /// ([`Run::finish`] takes this copy of the new series as the insert's proof), Remove deletes it.
    TailThere { tail: Value },
    /// The new series isn't there (looked for three times, or deleted), and the original was never
    /// cut. Remove has nothing to delete; a new series that shows up later is a visible duplicate.
    TailAbsent,
    /// The original provably wasn't cut. The new series, if it's still there, may go: Remove deletes
    /// it.
    CutNotDone,
    /// Google shows the original uncut at an etag a copy of the cut went out on, which a copy still
    /// on its way could change: only Remove can prove it, with a fence write first.
    CutUnproved,
    /// The new series is deleted, or wasn't there to delete: Remove's answer, and Check's when a
    /// rollback's delete that couldn't be proved turns out done.
    Removed,
    /// PM can't tell: look again later.
    Unknown,
}

/// What's known of the cut.
#[derive(Debug, Clone, PartialEq)]
enum Cut {
    /// Never sent: the insert before it is unknown.
    NotSent,
    /// Proved not done (the run's rollback couldn't delete the new series).
    Proved,
    /// Unknown, with every If-Match a copy of it went out on.
    Sent(Vec<String>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettlePhase {
    Start,
    /// The new series' `n`th look is out.
    TailLook(usize),
    TailPause(usize),
    /// A GET of the original is out.
    OriginalLook,
    /// The fence write is out.
    Fence,
    /// A GET of the original after the fence met a 412, or a 2xx that left Google at a cut etag.
    OriginalRelook,
    /// A GET of the new series before deleting it is out (`resent`: the delete went once more).
    DeleteLook {
        resent: bool,
    },
    /// The delete of the new series is out.
    Delete {
        resent: bool,
    },
    Over,
}

/// Settles a split a [`Run`] left partial: an unknown insert, an unknown cut, or a cut proved not
/// done whose rollback couldn't delete the new series (plan.rs: "Removing a new series is an undo like
/// any other"). It never sends the cut: that would turn Remove into Finish against the user's choice.
#[derive(Debug)]
pub struct Settle {
    plan: SeriesPlan,
    insert: usize,
    cut_step: usize,
    cut: Cut,
    action: SettleAction,
    phase: SettlePhase,
    pending: Option<Answer>,
    result: Option<Settled>,
}

impl Settle {
    /// Settle what `ran` left of `plan`, or `None` when it left no split to settle.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "first caller lands in C9a-3, settling a partial split"
        )
    )]
    pub fn new(plan: SeriesPlan, ran: &Ran, action: SettleAction) -> Option<Settle> {
        let insert = plan
            .steps
            .iter()
            .position(|s| matches!(s.op, Op::Insert { .. }))?;
        let cut_step = insert + 1;
        if !matches!(
            plan.steps.get(cut_step),
            Some(s) if s.stage == Stage::Commit && matches!(s.op, Op::Patch { .. })
        ) {
            return None;
        }
        if ran.ends().len() != plan.steps.len() {
            return None;
        }
        let cut = match ran {
            Ran::Unknown { step, .. } if *step == insert => Cut::NotSent,
            Ran::Unknown {
                step,
                sent_on,
                ends,
            } if *step == cut_step && matches!(ends.get(insert), Some(End::Done { .. })) => {
                Cut::Sent(sent_on.clone())
            }
            Ran::NotDone {
                rollback: Rollback::Unknown { step },
                ..
            } if *step == insert => Cut::Proved,
            _ => return None,
        };
        Some(Settle {
            plan,
            insert,
            cut_step,
            cut,
            action,
            phase: SettlePhase::Start,
            pending: None,
            result: None,
        })
    }

    fn advance(&mut self) -> Next<Settled> {
        loop {
            if let Some(result) = &self.result {
                return Next::Over(result.clone());
            }
            let answer = self.pending.take();
            let next = match (self.phase, answer) {
                (SettlePhase::Start, None) => Some(match self.cut {
                    Cut::NotSent | Cut::Proved => self.look_for_tail(1),
                    Cut::Sent(_) => self.look_at_original(SettlePhase::OriginalLook),
                }),
                (SettlePhase::TailLook(n), Some(a)) => Some(self.on_tail_look(n, a)),
                (SettlePhase::TailPause(n), None) => Some(self.look_for_tail(n + 1)),
                (SettlePhase::OriginalLook, Some(a)) => Some(self.on_original(a)),
                (SettlePhase::Fence, Some(a)) => Some(self.on_fence(a)),
                (SettlePhase::OriginalRelook, Some(a)) => Some(self.on_relook(a)),
                (SettlePhase::DeleteLook { resent }, Some(a)) => {
                    Some(self.on_delete_look(resent, a))
                }
                (SettlePhase::Delete { resent }, Some(a)) => Some(self.on_delete(resent, a)),
                _ => Some(self.over(Settled::Unknown)),
            };
            if let Some(next) = next {
                return next;
            }
        }
    }

    fn tail_undo(&self) -> Option<&Undo> {
        self.plan.steps.get(self.insert)?.undo.as_ref()
    }

    fn look_for_tail(&mut self, n: usize) -> Next<Settled> {
        match self.tail_undo().map(undo_target) {
            Some(request) => {
                self.phase = SettlePhase::TailLook(n);
                Next::Send(request)
            }
            None => self.over(Settled::Unknown),
        }
    }

    fn on_tail_look(&mut self, n: usize, answer: Answer) -> Next<Settled> {
        match answer {
            Answer::Reply {
                verdict: Verdict::Ok,
                body: Some(tail),
                ..
            } if self.plan.verdict(self.insert, Some(&tail)) == Applied::Yes => {
                match (self.action, &self.cut) {
                    (SettleAction::Remove, _) => self.delete_tail(tail, false),
                    (SettleAction::Check, Cut::Proved) => self.over(Settled::CutNotDone),
                    (SettleAction::Check, _) => self.over(Settled::TailThere { tail }),
                }
            }
            // Deleted: Google keeps a deleted event's id as a cancelled copy (`is_gone`), and the id
            // is PM's own, minted for this plan, so a late copy of the insert can only be refused.
            Answer::Reply {
                verdict: Verdict::Ok,
                body: Some(tail),
                ..
            } if is_gone(&tail) => self.over(match (self.action, &self.cut) {
                (SettleAction::Check, Cut::NotSent) => Settled::TailAbsent,
                _ => Settled::Removed,
            }),
            Answer::Reply {
                verdict: Verdict::Gone,
                ..
            } => match (&self.cut, LOOK_AGAIN.get(n - 1)) {
                // A rollback's delete that couldn't be proved: gone now, so it's done.
                (Cut::Proved, _) => self.over(Settled::Removed),
                (_, Some(pause)) => {
                    self.phase = SettlePhase::TailPause(n);
                    Next::Wait(*pause)
                }
                (_, None) => self.over(match self.action {
                    SettleAction::Check => Settled::TailAbsent,
                    SettleAction::Remove => Settled::Removed,
                }),
            },
            _ => self.over(Settled::Unknown),
        }
    }

    fn look_at_original(&mut self, phase: SettlePhase) -> Next<Settled> {
        match &self.plan.steps[self.cut_step].op {
            Op::Patch {
                calendar, event, ..
            } => {
                self.phase = phase;
                Next::Send(get_event(calendar, event))
            }
            _ => self.over(Settled::Unknown),
        }
    }

    fn sent_on(&self) -> &[String] {
        match &self.cut {
            Cut::Sent(sent_on) => sent_on,
            Cut::NotSent | Cut::Proved => &[],
        }
    }

    fn on_original(&mut self, answer: Answer) -> Next<Settled> {
        let fresh = match answer {
            Answer::Reply {
                verdict: Verdict::Ok,
                body: Some(b),
                ..
            } => Some(b),
            Answer::Reply {
                verdict: Verdict::Gone,
                ..
            } => None,
            _ => return self.over(Settled::Unknown),
        };
        match self.plan.verdict(self.cut_step, fresh.as_ref()) {
            Applied::Yes => match fresh {
                Some(original) => self.over(Settled::CutDone { original }),
                None => self.over(Settled::Unknown),
            },
            Applied::No => {
                // Gone (404/410) proves it, wherever the cut is; a copy without an etag proves
                // nothing.
                let at_a_cut_etag = match &fresh {
                    None => false,
                    Some(b) => match etag_of(b) {
                        Some(e) => self.sent_on().contains(&e),
                        None => return self.over(Settled::Unknown),
                    },
                };
                match (self.action, at_a_cut_etag, fresh) {
                    (SettleAction::Check, false, _) => self.over(Settled::CutNotDone),
                    (SettleAction::Check, true, _) => self.over(Settled::CutUnproved),
                    (SettleAction::Remove, false, _) => self.look_at_tail_to_delete(),
                    (SettleAction::Remove, true, Some(original)) => self.fence(&original),
                    (SettleAction::Remove, true, None) => self.over(Settled::Unknown),
                }
            }
            Applied::Unknown => self.over(Settled::Unknown),
        }
    }

    /// Move the original off the cut's etag with a write of PM's own private note, on that etag: once
    /// it lands, no copy of the cut can. The note names the etag it went out on, so it always changes
    /// something, whatever an earlier fence left there.
    fn fence(&mut self, original: &Value) -> Next<Settled> {
        let plan = match &self.plan.steps[self.insert].shows {
            Shows::Tail { plan, .. } => plan.as_str(),
            _ => "",
        };
        let marker = format!("{plan} {}", etag_of(original).unwrap_or_default());
        let Op::Patch { calendar, .. } = &self.plan.steps[self.cut_step].op else {
            return self.over(Settled::Unknown);
        };
        match fence_request(calendar, original, &marker) {
            Some(request) => {
                self.phase = SettlePhase::Fence;
                Next::Send(request)
            }
            None => self.over(Settled::Unknown),
        }
    }

    fn on_fence(&mut self, answer: Answer) -> Next<Settled> {
        match answer {
            Answer::Reply {
                verdict: Verdict::Ok,
                body: Some(b),
                ..
            } => match self.plan.verdict(self.cut_step, Some(&b)) {
                // Only once the fence moved the etag past every copy of the cut.
                Applied::No if etag_of(&b).is_some_and(|e| !self.sent_on().contains(&e)) => {
                    self.look_at_tail_to_delete()
                }
                Applied::No => self.look_at_original(SettlePhase::OriginalRelook),
                Applied::Yes => self.over(Settled::CutDone { original: b }),
                Applied::Unknown => self.over(Settled::Unknown),
            },
            // The etag moved: by the cut, or by someone else. Look and take the cut's verdict.
            Answer::Reply {
                verdict: Verdict::Conflict,
                maybe_applied: false,
                ..
            } => self.look_at_original(SettlePhase::OriginalRelook),
            _ => self.over(Settled::Unknown),
        }
    }

    fn on_relook(&mut self, answer: Answer) -> Next<Settled> {
        let fresh = match answer {
            Answer::Reply {
                verdict: Verdict::Ok,
                body: Some(b),
                ..
            } => b,
            _ => return self.over(Settled::Unknown),
        };
        match self.plan.verdict(self.cut_step, Some(&fresh)) {
            Applied::Yes => self.over(Settled::CutDone { original: fresh }),
            Applied::No if etag_of(&fresh).is_some_and(|e| !self.sent_on().contains(&e)) => {
                self.look_at_tail_to_delete()
            }
            _ => self.over(Settled::Unknown),
        }
    }

    fn look_at_tail_to_delete(&mut self) -> Next<Settled> {
        match self.tail_undo().map(undo_target) {
            Some(request) => {
                self.phase = SettlePhase::DeleteLook { resent: false };
                Next::Send(request)
            }
            None => self.over(Settled::Unknown),
        }
    }

    fn on_delete_look(&mut self, resent: bool, answer: Answer) -> Next<Settled> {
        match answer {
            Answer::Reply {
                verdict: Verdict::Gone,
                ..
            } => self.over(Settled::Removed),
            Answer::Reply {
                verdict: Verdict::Ok,
                body: Some(tail),
                ..
            } => self.delete_tail(tail, resent),
            _ => self.over(Settled::Unknown),
        }
    }

    /// Delete the new series at `tail`, Google's copy of it, by the undo's own rules: only while it
    /// is still this plan's new series, once more at most.
    fn delete_tail(&mut self, tail: Value, resent: bool) -> Next<Settled> {
        let Some(undo) = self.tail_undo().cloned() else {
            return self.over(Settled::Unknown);
        };
        if is_gone(&tail) {
            return self.over(Settled::Removed);
        }
        match etag_of(&tail) {
            Some(etag) if undo.resend_on(&tail) => {
                self.phase = SettlePhase::Delete { resent };
                Next::Send(undo.request(&etag, self.plan.steps[self.insert].notify))
            }
            _ => self.over(Settled::Unknown),
        }
    }

    fn on_delete(&mut self, resent: bool, answer: Answer) -> Next<Settled> {
        match answer {
            Answer::Reply {
                verdict: Verdict::Ok | Verdict::Gone,
                ..
            } => self.over(Settled::Removed),
            Answer::Reply {
                verdict,
                maybe_applied: false,
                ..
            } if refused(&verdict) => self.over(Settled::Unknown),
            _ if !resent => self.look_again_before_deleting(),
            _ => self.over(Settled::Unknown),
        }
    }

    fn look_again_before_deleting(&mut self) -> Next<Settled> {
        match self.tail_undo().map(undo_target) {
            Some(request) => {
                self.phase = SettlePhase::DeleteLook { resent: true };
                Next::Send(request)
            }
            None => self.over(Settled::Unknown),
        }
    }

    fn over(&mut self, result: Settled) -> Next<Settled> {
        self.phase = SettlePhase::Over;
        self.result = Some(result.clone());
        Next::Over(result)
    }
}

impl Machine for Settle {
    type Output = Settled;

    fn next(&mut self) -> Next<Settled> {
        self.advance()
    }

    fn answer(&mut self, answer: Answer) {
        self.pending = Some(answer);
    }
}

/// The fence write: PM's private note `pmFence` on the series `original`, on its etag, notify none.
/// Every existing private and shared property goes back with it, so it can't wipe another app's
/// whether Google merges or replaces the maps on a PATCH. Built here, not as an [`Op`]: a series
/// PATCH may not send `extendedProperties` (`Op::fits`).
pub fn fence_request(calendar: &str, original: &Value, marker: &str) -> Option<RequestPlan> {
    let id = original
        .get("id")
        .and_then(Value::as_str)
        .filter(|id| is_plain_segment(id))?;
    let etag = original.get("etag").and_then(Value::as_str)?;
    let props = |scope: &str| -> Map<String, Value> {
        original
            .pointer(&format!("/extendedProperties/{scope}"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default()
    };
    let mut private = props("private");
    private.insert("pmFence".into(), Value::from(marker));
    let shared = props("shared");
    let mut extended = json!({ "private": private });
    if !shared.is_empty() {
        extended["shared"] = Value::Object(shared);
    }
    Some(patch_event(
        calendar,
        id,
        etag,
        json!({ "extendedProperties": extended }),
        Notify::None,
        VersionFlags::default(),
    ))
}

#[cfg(test)]
mod tests {
    use super::super::fixtures::*;
    use super::super::plan::{plan_series_edit, Step};
    use super::super::scope::Scope;
    use super::*;

    /// Drive `m` to the end, answering each request with `answer`: the result, every request sent and
    /// every pause asked for.
    fn drive<M: Machine>(
        m: &mut M,
        mut answer: impl FnMut(&RequestPlan) -> Answer,
    ) -> (M::Output, Vec<RequestPlan>, Vec<Duration>) {
        let (mut sent, mut waits) = (Vec::new(), Vec::new());
        for _ in 0..200 {
            match m.next() {
                Next::Send(r) => {
                    let a = answer(&r);
                    sent.push(r);
                    m.answer(a);
                }
                Next::Wait(d) => waits.push(d),
                Next::Over(out) => return (out, sent, waits),
            }
        }
        panic!("the machine never finished");
    }

    fn ok(v: Value) -> Answer {
        Answer::Reply {
            verdict: Verdict::Ok,
            body: Some(v),
            date: None,
            maybe_applied: false,
        }
    }

    fn date() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-10-17T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    /// A 2xx with no body, the way Google answers a DELETE.
    fn deleted() -> Answer {
        Answer::Reply {
            verdict: Verdict::Ok,
            body: None,
            date: Some(date()),
            maybe_applied: false,
        }
    }

    fn status(verdict: Verdict) -> Answer {
        Answer::Reply {
            verdict,
            body: None,
            date: None,
            maybe_applied: false,
        }
    }

    /// The sender's last answer after an earlier attempt met a server error.
    fn after_5xx(verdict: Verdict) -> Answer {
        Answer::Reply {
            verdict,
            body: None,
            date: None,
            maybe_applied: true,
        }
    }

    fn lost() -> Answer {
        Answer::Lost { retried: false }
    }

    fn refusals() -> Vec<Verdict> {
        vec![
            Verdict::BadRequest("no".into()),
            Verdict::ReadOnly,
            Verdict::NotOrganizer,
            Verdict::InsufficientScope,
            Verdict::QuotaExceeded,
            Verdict::Reauth,
            Verdict::RateLimited,
        ]
    }

    /// `base` with what `step` sends landed on it, at `etag`.
    fn landed(base: &Value, step: &Step, etag: &str) -> Value {
        let mut v = base.clone();
        if let Shows::Fields { sent, .. } = &step.shows {
            for (k, x) in sent {
                v[k] = x.clone();
            }
        }
        v["etag"] = json!(etag);
        v
    }

    fn at_etag(base: &Value, etag: &str) -> Value {
        let mut v = base.clone();
        v["etag"] = json!(etag);
        v
    }

    /// Google's copy of the new series a split inserts, at `etag`.
    fn tail_of(plan: &SeriesPlan, etag: &str) -> Value {
        let Op::Insert { body, id, .. } = &plan.steps[0].op else {
            panic!("not a split");
        };
        let mut v = Value::Object(body.clone());
        v["id"] = json!(id);
        v["etag"] = json!(etag);
        v["status"] = json!("confirmed");
        v
    }

    fn to(r: &RequestPlan, event: &str) -> bool {
        r.url.contains(&format!("/events/{event}?")) || r.url.ends_with(&format!("/events/{event}"))
    }

    fn deletes_the_tail(sent: &[RequestPlan]) -> bool {
        sent.iter()
            .any(|r| r.method == Method::Delete && to(r, TAIL))
    }

    /// A renamed occurrence after the split, for a plan with a carry and its cancel.
    fn renamed() -> Value {
        with(plain("2026-11-02T09:00:00Z"), "summary", json!("Off-site"))
    }

    fn split_case(exceptions: Vec<Value>) -> (Case, SeriesPlan) {
        let case = Case::new(weekly(), monday_19()).exceptions(exceptions);
        let plan = case.edit(Scope::Following, title("Standup v2")).unwrap();
        (case, plan)
    }

    fn this_case() -> (Case, SeriesPlan) {
        let case = Case::new(weekly(), monday_19());
        let plan = case.edit(Scope::This, title("Moved")).unwrap();
        (case, plan)
    }

    fn instance_etag(case: &Case) -> String {
        etag_of(case.instance.json()).unwrap()
    }

    // --- Run: the steps' own answers ---

    #[test]
    fn a_plan_with_a_key_its_write_may_not_send_never_starts() {
        let (_, mut plan) = this_case();
        if let Op::Patch { body, .. } = &mut plan.steps[0].op {
            body.insert("attendees".into(), json!([]));
        }
        assert_eq!(Run::new(plan).err(), Some(Unfit));
    }

    #[test]
    fn this_event_lands_on_a_2xx_holding_what_was_sent() {
        let (case, plan) = this_case();
        let reply = landed(case.instance.json(), &plan.steps[0], "\"i2\"");
        let mut run = Run::new(plan.clone()).unwrap();
        assert_eq!(run.plan(), &plan);
        let (ran, sent, _) = drive(&mut run, |_| ok(reply.clone()));
        assert_eq!(sent, vec![plan.steps[0].op.request(Notify::None).unwrap()]);
        assert_eq!(
            ran,
            Ran::Done {
                ends: vec![End::Done {
                    reply: Some(reply),
                    date: None
                }]
            }
        );
    }

    /// L-R5: Google answered the cut 200 and kept the old rule. That proves it not done, so the new
    /// series goes, on its own reply's etag, with the step's own `sendUpdates`.
    #[test]
    fn a_2xx_holding_the_old_rule_proves_the_cut_ignored_and_deletes_the_new_series() {
        let (case, plan) = split_case(vec![renamed()]);
        let tail = tail_of(&plan, "\"t1\"");
        let (ran, sent, _) = drive(&mut Run::new(plan).unwrap(), |r| match r.method {
            Method::Insert => ok(tail.clone()),
            Method::Patch => ok(at_etag(case.master.json(), "\"m2\"")),
            Method::Delete => deleted(),
            Method::Get => panic!("nothing to look at: {}", r.url),
        });
        assert_eq!(
            ran,
            Ran::NotDone {
                step: 1,
                why: Why::Ignored,
                rollback: Rollback::Done,
                ends: vec![End::Undone, End::NotRun, End::NotRun, End::NotRun],
            }
        );
        let undo = sent.last().unwrap();
        assert_eq!(undo.method, Method::Delete);
        assert!(to(undo, TAIL) && undo.url.contains("sendUpdates=none"));
        assert_eq!(undo.if_match.as_deref(), Some("\"t1\""));
    }

    #[test]
    fn a_412_whose_get_shows_the_change_is_done() {
        let (case, plan) = this_case();
        let reply = landed(case.instance.json(), &plan.steps[0], "\"i2\"");
        let (ran, sent, _) = drive(&mut Run::new(plan).unwrap(), |r| match r.method {
            Method::Patch => status(Verdict::Conflict),
            _ => ok(reply.clone()),
        });
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1].method, Method::Get);
        assert_eq!(
            ran,
            Ran::Done {
                ends: vec![End::Done {
                    reply: Some(reply),
                    date: None
                }]
            }
        );
    }

    #[test]
    fn a_412_with_the_watched_fields_unchanged_goes_once_more_on_the_fresh_etag() {
        let (case, plan) = this_case();
        let reply = landed(case.instance.json(), &plan.steps[0], "\"i3\"");
        let mut patches = 0;
        let (ran, sent, _) = drive(&mut Run::new(plan).unwrap(), |r| match r.method {
            Method::Patch => {
                patches += 1;
                if patches == 1 {
                    status(Verdict::Conflict)
                } else {
                    ok(reply.clone())
                }
            }
            _ => ok(at_etag(case.instance.json(), "\"i9\"")),
        });
        assert!(matches!(ran, Ran::Done { .. }));
        assert_eq!(sent.len(), 3);
        assert_eq!(sent[0].if_match, Some(instance_etag(&case)));
        assert_eq!(sent[2].if_match.as_deref(), Some("\"i9\""));
        assert_eq!(sent[2].body, sent[0].body);
    }

    /// C9 design review: the cut watches every field the new series copied (AS10), so one changed
    /// in Google since is proof, and the insert is undone.
    #[test]
    fn a_412_with_a_watched_field_changed_is_proof_and_undoes_the_insert() {
        let (case, plan) = split_case(Vec::new());
        let tail = tail_of(&plan, "\"t1\"");
        let renamed = at_etag(
            &with(case.master.json().clone(), "summary", json!("Standup v3")),
            "\"m3\"",
        );
        let (ran, sent, _) = drive(&mut Run::new(plan).unwrap(), |r| match r.method {
            Method::Insert => ok(tail.clone()),
            Method::Patch => status(Verdict::Conflict),
            Method::Get => ok(renamed.clone()),
            Method::Delete => deleted(),
        });
        assert_eq!(
            ran,
            Ran::NotDone {
                step: 1,
                why: Why::Conflict(vec!["summary".into()]),
                rollback: Rollback::Done,
                ends: vec![End::Undone, End::NotRun],
            }
        );
        assert_eq!(sent.iter().filter(|r| r.method == Method::Patch).count(), 1);
    }

    #[test]
    fn a_second_412_after_the_resend_is_a_conflict() {
        let (case, plan) = this_case();
        let mut gets = 0;
        let (ran, sent, _) = drive(&mut Run::new(plan).unwrap(), |r| match r.method {
            Method::Patch => status(Verdict::Conflict),
            _ => {
                gets += 1;
                ok(at_etag(case.instance.json(), &format!("\"i9{gets}\"")))
            }
        });
        assert_eq!(
            ran,
            Ran::NotDone {
                step: 0,
                why: Why::Conflict(Vec::new()),
                rollback: Rollback::Nothing,
                ends: vec![End::NotRun],
            }
        );
        assert_eq!(sent.len(), 4);
        assert_eq!(sent[2].if_match.as_deref(), Some("\"i91\""));
    }

    #[test]
    fn a_lost_answer_is_no_proof_until_the_same_request_went_once_more_on_its_own_if_match() {
        let (case, plan) = this_case();
        let old = case.instance.json().clone();
        let (ran, sent, _) = drive(&mut Run::new(plan).unwrap(), |r| match r.method {
            Method::Patch => lost(),
            _ => ok(old.clone()),
        });
        let etag = instance_etag(&case);
        assert_eq!(
            ran,
            Ran::Unknown {
                step: 0,
                sent_on: vec![etag.clone()],
                ends: vec![End::NotRun],
            }
        );
        let methods: Vec<Method> = sent.iter().map(|r| r.method).collect();
        assert_eq!(
            methods,
            [Method::Patch, Method::Get, Method::Patch, Method::Get]
        );
        assert_eq!(sent[2], sent[0]);
    }

    #[test]
    fn a_late_first_copy_answers_the_resend_412_and_the_get_shows_it_landed() {
        let (case, plan) = this_case();
        let old = case.instance.json().clone();
        let reply = landed(&old, &plan.steps[0], "\"i2\"");
        let (mut patches, mut gets) = (0, 0);
        let (ran, _, _) = drive(&mut Run::new(plan).unwrap(), |r| match r.method {
            Method::Patch => {
                patches += 1;
                if patches == 1 {
                    lost()
                } else {
                    status(Verdict::Conflict)
                }
            }
            _ => {
                gets += 1;
                ok(if gets == 1 {
                    old.clone()
                } else {
                    reply.clone()
                })
            }
        });
        assert!(matches!(ran, Ran::Done { .. }), "{ran:?}");
    }

    #[test]
    fn a_5xx_the_sender_already_retried_is_not_sent_a_third_time() {
        let (case, plan) = this_case();
        let old = case.instance.json().clone();
        let (ran, sent, _) = drive(&mut Run::new(plan).unwrap(), |r| match r.method {
            Method::Patch => after_5xx(Verdict::ServerError),
            _ => ok(old.clone()),
        });
        assert!(matches!(ran, Ran::Unknown { step: 0, .. }));
        assert_eq!(sent.iter().filter(|r| r.method == Method::Patch).count(), 1);
    }

    /// Review H1: the sender's retry after a server error came back refused. That refusal says
    /// nothing about the first copy, which may have landed: look, never roll back on it.
    #[test]
    fn a_server_error_then_a_refusal_is_checked_never_rolled_back() {
        for refusal in refusals() {
            let (case, plan) = split_case(Vec::new());
            let tail = tail_of(&plan, "\"t1\"");
            let cut = landed(case.master.json(), &plan.steps[1], "\"m2\"");
            let (ran, sent, _) = drive(&mut Run::new(plan).unwrap(), |r| match r.method {
                Method::Insert => ok(tail.clone()),
                Method::Patch => after_5xx(refusal.clone()),
                Method::Get => ok(cut.clone()),
                Method::Delete => deleted(),
            });
            assert!(matches!(ran, Ran::Done { .. }), "{refusal:?}: {ran:?}");
            assert!(!deletes_the_tail(&sent), "{refusal:?}");

            // Google still shows the original uncut at the cut's etag: the refused retry never
            // reached it, so the cut goes once more; then nothing can be proved.
            let (case, plan) = split_case(Vec::new());
            let old = case.master.json().clone();
            let mut patches = 0;
            let (ran, sent, _) = drive(&mut Run::new(plan.clone()).unwrap(), |r| match r.method {
                Method::Insert => ok(tail.clone()),
                Method::Patch => {
                    patches += 1;
                    if patches == 1 {
                        after_5xx(refusal.clone())
                    } else {
                        lost()
                    }
                }
                Method::Get => ok(old.clone()),
                Method::Delete => deleted(),
            });
            assert!(matches!(ran, Ran::Unknown { step: 1, .. }), "{ran:?}");
            assert_eq!(patches, 2, "{refusal:?}");
            assert!(!deletes_the_tail(&sent));
        }
    }

    #[test]
    fn a_lost_retry_after_a_server_error_is_unknown_never_rolled_back() {
        let (case, plan) = split_case(Vec::new());
        let tail = tail_of(&plan, "\"t1\"");
        let old = case.master.json().clone();
        let (ran, sent, _) = drive(&mut Run::new(plan).unwrap(), |r| match r.method {
            Method::Insert => ok(tail.clone()),
            Method::Patch => Answer::Lost { retried: true },
            Method::Get => ok(old.clone()),
            Method::Delete => deleted(),
        });
        assert!(matches!(ran, Ran::Unknown { step: 1, .. }));
        assert_eq!(sent.iter().filter(|r| r.method == Method::Patch).count(), 1);
        assert!(!deletes_the_tail(&sent));
    }

    /// plan.rs's first rule, exhaustively for the cut: no script of answers that never proves it not
    /// done deletes the new series, however the answers fall, Google showing what was there at the
    /// cut's own etag or at a newer one. At most four requests of the cut leave the machine, each on
    /// an etag where Google showed it not landed.
    #[test]
    fn no_answer_without_proof_ever_deletes_the_new_series() {
        let (case, plan) = split_case(Vec::new());
        let tail = tail_of(&plan, "\"t1\"");
        let old = case.master.json().clone();
        let third_rule = at_etag(
            &with(old.clone(), "recurrence", json!(["RRULE:FREQ=DAILY"])),
            "\"m5\"",
        );
        let writes = [
            lost(),
            Answer::Lost { retried: true },
            after_5xx(Verdict::ServerError),
            after_5xx(Verdict::RateLimited),
            after_5xx(Verdict::Conflict),
            after_5xx(Verdict::Reauth),
            status(Verdict::Other(409, "conflict".into())),
            status(Verdict::Duplicate),
            Answer::Reply {
                verdict: Verdict::Ok,
                body: None,
                date: None,
                maybe_applied: false,
            },
        ];
        let gets = [
            ok(old.clone()),
            lost(),
            status(Verdict::ServerError),
            ok(third_rule),
            // A guest's reply moved the etag: what was there, at an etag no copy went out on.
            ok(at_etag(&old, "\"m7\"")),
        ];
        let (mut runs, mut four) = (0, 0);
        for w in 0..writes.len().pow(3) {
            for g in 0..gets.len().pow(3) {
                let pick = |n: usize, i: usize, base: usize| (n / base.pow(i as u32)) % base;
                let (mut wi, mut gi) = (0, 0);
                let mut shown = vec!["\"m1\"".to_string()];
                let (ran, sent, _) =
                    drive(&mut Run::new(plan.clone()).unwrap(), |r| match r.method {
                        Method::Insert => ok(tail.clone()),
                        Method::Patch => {
                            let a = if wi < 3 {
                                writes[pick(w, wi, writes.len())].clone()
                            } else {
                                lost()
                            };
                            wi += 1;
                            a
                        }
                        Method::Get => {
                            let a = if gi < 3 {
                                gets[pick(g, gi, gets.len())].clone()
                            } else {
                                lost()
                            };
                            gi += 1;
                            if let Answer::Reply { body: Some(b), .. } = &a {
                                if b["recurrence"] == old["recurrence"] {
                                    shown.extend(etag_of(b));
                                }
                            }
                            a
                        }
                        Method::Delete => deleted(),
                    });
                runs += 1;
                assert!(!deletes_the_tail(&sent), "{w}/{g}: {sent:?}");
                assert!(
                    matches!(ran, Ran::Unknown { step: 1, .. }),
                    "{w}/{g}: {ran:?}"
                );
                let cuts: Vec<&RequestPlan> =
                    sent.iter().filter(|r| r.method == Method::Patch).collect();
                assert!(cuts.len() <= 4, "{w}/{g}");
                four += usize::from(cuts.len() == 4);
                for c in &cuts {
                    let on = c.if_match.as_ref().unwrap();
                    assert!(shown.contains(on), "{w}/{g}");
                    // The first copy, or a resend on a newer etag, and one once-more on it.
                    let per_etag = cuts
                        .iter()
                        .filter(|x| x.if_match.as_ref() == Some(on))
                        .count();
                    assert!(per_etag <= 2, "{w}/{g}: {per_etag} on {on}");
                }
            }
        }
        assert_eq!(runs, 729 * 125);
        // The bound is reached: a resend on the newer etag, and its own once-more.
        assert!(four > 0);
    }

    /// Review finding: an answer classify can't read, or a 412 the GET contradicts, then Google at
    /// the very etag the copy went out on, is no proof, however often it happens.
    #[test]
    fn what_was_there_at_the_cuts_own_etag_is_never_proof() {
        for answer in [
            status(Verdict::Other(409, "conflict".into())),
            status(Verdict::Duplicate),
            status(Verdict::Conflict),
            // A server error the sender didn't flag is in doubt all the same.
            status(Verdict::ServerError),
        ] {
            let (case, plan) = split_case(Vec::new());
            let tail = tail_of(&plan, "\"t1\"");
            let old = case.master.json().clone();
            let (ran, sent, _) = drive(&mut Run::new(plan).unwrap(), |r| match r.method {
                Method::Insert => ok(tail.clone()),
                Method::Patch => answer.clone(),
                Method::Get => ok(old.clone()),
                Method::Delete => deleted(),
            });
            assert!(
                matches!(ran, Ran::Unknown { step: 1, .. }),
                "{answer:?}: {ran:?}"
            );
            assert!(!deletes_the_tail(&sent), "{answer:?}");
            let cuts: Vec<&RequestPlan> =
                sent.iter().filter(|r| r.method == Method::Patch).collect();
            assert_eq!(cuts.len(), 2, "{answer:?}");
            assert!(cuts.iter().all(|c| c.if_match.as_deref() == Some("\"m1\"")));
        }
    }

    /// Review finding: the cut was refused with a 412 and nothing in doubt, and someone else has
    /// changed the rule since: no copy can land, and the 412 rule calls it a conflict.
    #[test]
    fn a_412_then_someone_elses_change_is_proof() {
        let (case, plan) = split_case(Vec::new());
        let tail = tail_of(&plan, "\"t1\"");
        let daily = at_etag(
            &with(
                case.master.json().clone(),
                "recurrence",
                json!(["RRULE:FREQ=DAILY"]),
            ),
            "\"m5\"",
        );
        let answers = |patch: Answer| {
            let (tail, daily) = (tail.clone(), daily.clone());
            move |r: &RequestPlan| match r.method {
                Method::Insert => ok(tail.clone()),
                Method::Patch => patch.clone(),
                Method::Get => ok(daily.clone()),
                Method::Delete => deleted(),
            }
        };
        let (ran, sent, _) = drive(
            &mut Run::new(plan.clone()).unwrap(),
            answers(status(Verdict::Conflict)),
        );
        assert_eq!(
            ran,
            Ran::NotDone {
                step: 1,
                why: Why::Conflict(vec!["recurrence".into()]),
                rollback: Rollback::Done,
                ends: vec![End::Undone, End::NotRun],
            }
        );
        assert!(deletes_the_tail(&sent));
        // After a lost copy, the same GET may show that copy landed and then changed: unknown.
        let (ran, sent, _) = drive(&mut Run::new(plan).unwrap(), answers(lost()));
        assert!(matches!(ran, Ran::Unknown { step: 1, .. }));
        assert!(!deletes_the_tail(&sent));
    }

    // --- Run: inserts ---

    #[test]
    fn an_unknown_insert_never_sends_the_cut() {
        let (_, plan) = split_case(Vec::new());
        let (ran, sent, _) = drive(&mut Run::new(plan).unwrap(), |_| lost());
        assert!(matches!(ran, Ran::Unknown { step: 0, .. }));
        assert!(sent.iter().all(|r| r.method != Method::Patch));
    }

    #[test]
    fn a_409_is_proved_by_this_plans_note() {
        let (case, plan) = split_case(Vec::new());
        let tail = tail_of(&plan, "\"t1\"");
        let cut = landed(case.master.json(), &plan.steps[1], "\"m2\"");
        let (ran, sent, _) = drive(&mut Run::new(plan).unwrap(), |r| match r.method {
            Method::Insert => status(Verdict::Duplicate),
            Method::Get => ok(tail.clone()),
            Method::Patch => ok(cut.clone()),
            Method::Delete => deleted(),
        });
        assert!(matches!(ran, Ran::Done { .. }));
        assert!(to(&sent[1], TAIL) && sent[1].method == Method::Get);
    }

    #[test]
    fn a_409_under_another_plans_note_is_unknown() {
        let (_, plan) = split_case(Vec::new());
        let mut other = tail_of(&plan, "\"t1\"");
        other["extendedProperties"]["private"]["pmSplitPlan"] = json!("another plan");
        let (ran, sent, _) = drive(&mut Run::new(plan).unwrap(), |r| match r.method {
            Method::Insert => status(Verdict::Duplicate),
            _ => ok(other.clone()),
        });
        assert!(matches!(ran, Ran::Unknown { step: 0, .. }));
        assert_eq!(sent.len(), 2);
    }

    /// A9: a lost insert's new series isn't there yet. Look again after a second, then post it again
    /// under the same id, never a new one.
    #[test]
    fn a_lost_insert_is_looked_for_twice_then_posted_again_under_the_same_id() {
        let (case, plan) = split_case(Vec::new());
        let tail = tail_of(&plan, "\"t1\"");
        let cut = landed(case.master.json(), &plan.steps[1], "\"m2\"");
        let mut inserts = 0;
        let (ran, sent, waits) = drive(&mut Run::new(plan).unwrap(), |r| match r.method {
            Method::Insert => {
                inserts += 1;
                if inserts == 1 {
                    lost()
                } else {
                    ok(tail.clone())
                }
            }
            Method::Get => status(Verdict::Gone),
            Method::Patch => ok(cut.clone()),
            Method::Delete => deleted(),
        });
        assert!(matches!(ran, Ran::Done { .. }), "{ran:?}");
        assert_eq!(waits, vec![Duration::from_secs(1)]);
        let posts: Vec<&RequestPlan> = sent.iter().filter(|r| r.method == Method::Insert).collect();
        assert_eq!(posts.len(), 2);
        assert_eq!(posts[0], posts[1]);
        let methods: Vec<Method> = sent.iter().take(4).map(|r| r.method).collect();
        assert_eq!(
            methods,
            [Method::Insert, Method::Get, Method::Get, Method::Insert]
        );
    }

    #[test]
    fn a_reposted_insert_lost_again_is_unknown() {
        let (_, plan) = split_case(Vec::new());
        let (ran, sent, _) = drive(&mut Run::new(plan).unwrap(), |r| match r.method {
            Method::Insert => lost(),
            _ => status(Verdict::Gone),
        });
        assert!(matches!(ran, Ran::Unknown { step: 0, .. }));
        assert_eq!(
            sent.iter().filter(|r| r.method == Method::Insert).count(),
            2
        );
    }

    // --- Run: deletes and cancels ---

    #[test]
    fn a_2xx_delete_is_done_and_keeps_googles_date() {
        let case = Case::new(weekly(), monday_19());
        let plan = case.delete(Scope::All).unwrap();
        let (ran, _, _) = drive(&mut Run::new(plan).unwrap(), |_| deleted());
        assert_eq!(
            ran,
            Ran::Done {
                ends: vec![End::Done {
                    reply: None,
                    date: Some(date())
                }]
            }
        );
    }

    #[test]
    fn a_cancel_answered_410_is_done() {
        let case = Case::new(weekly(), monday_19());
        let plan = case.delete(Scope::This).unwrap();
        let (ran, sent, _) = drive(&mut Run::new(plan).unwrap(), |_| status(Verdict::Gone));
        assert_eq!(
            ran,
            Ran::Done {
                ends: vec![End::Done {
                    reply: None,
                    date: None
                }]
            }
        );
        assert_eq!(sent.len(), 1);
    }

    #[test]
    fn a_patch_answered_404_is_not_done() {
        let (_, plan) = this_case();
        let (ran, _, _) = drive(&mut Run::new(plan).unwrap(), |_| status(Verdict::Gone));
        assert_eq!(
            ran,
            Ran::NotDone {
                step: 0,
                why: Why::Gone,
                rollback: Rollback::Nothing,
                ends: vec![End::NotRun],
            }
        );
    }

    #[test]
    fn every_final_refusal_is_proof_of_not_done() {
        for refusal in refusals() {
            let (_, plan) = split_case(Vec::new());
            let tail = tail_of(&plan, "\"t1\"");
            let (ran, sent, _) = drive(&mut Run::new(plan).unwrap(), |r| match r.method {
                Method::Insert => ok(tail.clone()),
                Method::Patch => status(refusal.clone()),
                Method::Delete => deleted(),
                Method::Get => panic!("nothing to look at"),
            });
            assert_eq!(
                ran,
                Ran::NotDone {
                    step: 1,
                    why: Why::Refused(refusal.clone()),
                    rollback: Rollback::Done,
                    ends: vec![End::Undone, End::NotRun],
                },
                "{refusal:?}"
            );
            assert!(deletes_the_tail(&sent));
        }
    }

    #[test]
    fn duplicate_or_other_on_a_patch_is_checked_then_unknown_or_done() {
        let (case, plan) = this_case();
        let reply = landed(case.instance.json(), &plan.steps[0], "\"i2\"");
        let (ran, _, _) = drive(&mut Run::new(plan.clone()).unwrap(), |r| match r.method {
            Method::Patch => status(Verdict::Other(418, "teapot".into())),
            _ => lost(),
        });
        assert!(matches!(ran, Ran::Unknown { step: 0, .. }));
        let (ran, _, _) = drive(&mut Run::new(plan).unwrap(), |r| match r.method {
            Method::Patch => status(Verdict::Duplicate),
            _ => ok(reply.clone()),
        });
        assert!(matches!(ran, Ran::Done { .. }));
    }

    // --- Run: who Google emails ---

    /// A3, R7b: every step, and every undo, sends the `sendUpdates` its plan stamped on it.
    #[test]
    fn every_step_and_undo_sends_its_own_send_updates() {
        for notify in [Notify::All, Notify::ExternalOnly, Notify::None] {
            let param = format!("sendUpdates={}", notify.as_param());
            let case = Case::new(weekly(), monday_19()).exceptions(vec![renamed()]);
            let plan = plan_series_edit(
                Scope::Following,
                &case.input(),
                &title("Standup v2"),
                TAIL,
                notify,
            )
            .unwrap();
            // Rolled back: the insert, the cut and the undo.
            let tail = tail_of(&plan, "\"t1\"");
            let (_, sent, _) = drive(&mut Run::new(plan.clone()).unwrap(), |r| match r.method {
                Method::Insert => ok(tail.clone()),
                Method::Patch => ok(case.master.json().clone()),
                Method::Delete => deleted(),
                Method::Get => panic!("nothing to look at"),
            });
            assert_eq!(sent.len(), 3);
            assert!(sent.iter().all(|r| r.url.contains(&param)), "{sent:?}");
            // Finished: the carry and the cancel too.
            let (ran, sent, _) = drive(&mut Run::new(plan.clone()).unwrap(), |r| {
                finished_split(&case, &plan, &tail, r)
            });
            assert!(matches!(ran, Ran::Done { .. }));
            let writes: Vec<&RequestPlan> =
                sent.iter().filter(|r| r.method != Method::Get).collect();
            assert_eq!(writes.len(), 4);
            assert!(writes.iter().all(|r| r.url.contains(&param)), "{writes:?}");

            // Left partial, then removed: Settle's delete of the new series too.
            let (_, sent_cut, _) =
                drive(&mut Run::new(plan.clone()).unwrap(), |r| match r.method {
                    Method::Insert => ok(tail.clone()),
                    _ => Answer::Lost { retried: true },
                });
            let ran = Ran::Unknown {
                step: 1,
                sent_on: vec!["\"m1\"".into()],
                ends: vec![
                    End::Done {
                        reply: Some(tail.clone()),
                        date: None,
                    },
                    End::NotRun,
                    End::NotRun,
                    End::NotRun,
                ],
            };
            assert!(sent_cut
                .iter()
                .all(|r| r.url.contains(&param) || r.method == Method::Get));
            let mut settle = Settle::new(plan.clone(), &ran, SettleAction::Remove).unwrap();
            let (settled, sent, _) = drive(&mut settle, |r| match r.method {
                Method::Get if to(r, "abc") => ok(at_etag(case.master.json(), "\"m7\"")),
                Method::Get => ok(tail.clone()),
                _ => deleted(),
            });
            assert_eq!(settled, Settled::Removed);
            assert!(sent.last().unwrap().url.contains(&param));

            // A half-landed move put back, on a series retimed with this `sendUpdates`.
            let case = Case::new(weekly(), monday_19());
            let draft = at("09:00", "2026-10-20");
            let plan = plan_series_edit(Scope::All, &case.input(), &draft, TAIL, notify).unwrap();
            let old = case.master.json().clone();
            let mut half = landed(&old, &plan.steps[0], "\"m2\"");
            half["recurrence"] = old["recurrence"].clone();
            let (ran, sent, _) = drive(&mut Run::new(plan).unwrap(), |r| match r.method {
                Method::Patch if r.if_match.as_deref() == Some("\"m1\"") => ok(half.clone()),
                Method::Patch => ok(at_etag(&old, "\"m3\"")),
                _ => ok(half.clone()),
            });
            assert!(matches!(
                ran,
                Ran::NotDone {
                    why: Why::PutBack,
                    ..
                }
            ));
            let patches: Vec<&RequestPlan> =
                sent.iter().filter(|r| r.method == Method::Patch).collect();
            assert_eq!(patches.len(), 2);
            assert!(
                patches.iter().all(|r| r.url.contains(&param)),
                "{patches:?}"
            );
        }
    }

    /// Google's answers to a split that goes through: the new series, the cut, the carry's occurrence
    /// found and changed, the original's copy cancelled.
    fn finished_split(case: &Case, plan: &SeriesPlan, tail: &Value, r: &RequestPlan) -> Answer {
        let occurrence = json!({
            "id": "q1", "etag": "\"q\"", "status": "confirmed",
            "originalStartTime": { "dateTime": "2026-11-02T09:00:00Z" },
            "summary": "Standup v2",
        });
        match r.method {
            Method::Insert => ok(tail.clone()),
            Method::Get => ok(json!({ "items": [occurrence] })),
            Method::Patch if to(r, "abc") => {
                ok(landed(case.master.json(), &plan.steps[1], "\"m2\""))
            }
            Method::Patch if to(r, "q1") => ok(landed(&occurrence, &plan.steps[2], "\"q2\"")),
            Method::Patch => ok(with(renamed(), "status", json!("cancelled"))),
            Method::Delete => deleted(),
        }
    }

    // --- Run: undoing ---

    #[test]
    fn an_undo_delete_meeting_404_is_done() {
        let (case, plan) = split_case(Vec::new());
        let tail = tail_of(&plan, "\"t1\"");
        let (ran, _, _) = drive(&mut Run::new(plan).unwrap(), |r| match r.method {
            Method::Insert => ok(tail.clone()),
            Method::Patch => ok(case.master.json().clone()),
            Method::Delete => status(Verdict::Gone),
            Method::Get => panic!("nothing to look at"),
        });
        assert!(matches!(
            ran,
            Ran::NotDone {
                rollback: Rollback::Done,
                ..
            }
        ));
    }

    /// An undo's own rule: on a 412 it goes once more, only while the event is still this plan's new
    /// series.
    #[test]
    fn an_undo_412_goes_once_more_only_while_it_is_still_this_plans_new_series() {
        let (case, plan) = split_case(Vec::new());
        let tail = tail_of(&plan, "\"t1\"");
        let mut deletes = 0;
        let (ran, sent, _) = drive(&mut Run::new(plan.clone()).unwrap(), |r| match r.method {
            Method::Insert => ok(tail.clone()),
            Method::Patch => ok(case.master.json().clone()),
            Method::Delete => {
                deletes += 1;
                if deletes == 1 {
                    status(Verdict::Conflict)
                } else {
                    deleted()
                }
            }
            Method::Get => ok(at_etag(&tail, "\"t2\"")),
        });
        assert!(matches!(
            ran,
            Ran::NotDone {
                rollback: Rollback::Done,
                ..
            }
        ));
        assert_eq!(sent.last().unwrap().if_match.as_deref(), Some("\"t2\""));

        let mut theirs = at_etag(&tail, "\"t2\"");
        theirs["extendedProperties"]["private"]
            .as_object_mut()
            .unwrap()
            .retain(|k, _| k != "pmSplitPlan");
        let (ran, sent, _) = drive(&mut Run::new(plan).unwrap(), |r| match r.method {
            Method::Insert => ok(tail.clone()),
            Method::Patch => ok(case.master.json().clone()),
            Method::Delete => status(Verdict::Conflict),
            Method::Get => ok(theirs.clone()),
        });
        assert!(matches!(
            ran,
            Ran::NotDone {
                rollback: Rollback::Unknown { step: 0 },
                ..
            }
        ));
        assert_eq!(
            sent.iter().filter(|r| r.method == Method::Delete).count(),
            1
        );
    }

    #[test]
    fn an_undo_that_cant_be_proved_reports_rollback_unknown() {
        let (case, plan) = split_case(Vec::new());
        let tail = tail_of(&plan, "\"t1\"");
        let (ran, _, _) = drive(&mut Run::new(plan).unwrap(), |r| match r.method {
            Method::Insert => ok(tail.clone()),
            Method::Patch => ok(case.master.json().clone()),
            Method::Delete => status(Verdict::ReadOnly),
            Method::Get => panic!("nothing to look at"),
        });
        assert_eq!(
            ran,
            Ran::NotDone {
                step: 1,
                why: Why::Ignored,
                rollback: Rollback::Unknown { step: 0 },
                ends: vec![
                    End::Done {
                        reply: Some(tail),
                        date: None
                    },
                    End::NotRun
                ],
            }
        );
    }

    // --- Run: a move Google took half of ---

    fn all_to_tuesday() -> (Case, SeriesPlan) {
        let case = Case::new(weekly(), monday_19());
        let plan = case.edit(Scope::All, at("09:00", "2026-10-20")).unwrap();
        (case, plan)
    }

    /// DS8: Google took the new start and kept the old rule. Only then are the times and rule put
    /// back, on the GET's etag with the step's own `sendUpdates`.
    #[test]
    fn restore_master_only_when_google_took_half_the_move() {
        let (case, plan) = all_to_tuesday();
        let old = case.master.json().clone();
        let mut half = landed(&old, &plan.steps[0], "\"m2\"");
        half["recurrence"] = old["recurrence"].clone();
        let restored = at_etag(&old, "\"m3\"");
        let (ran, sent, _) = drive(&mut Run::new(plan).unwrap(), |r| match r.method {
            Method::Patch if r.if_match.as_deref() == Some("\"m1\"") => ok(half.clone()),
            Method::Patch => ok(restored.clone()),
            _ => ok(half.clone()),
        });
        assert_eq!(
            ran,
            Ran::NotDone {
                step: 0,
                why: Why::PutBack,
                rollback: Rollback::Nothing,
                ends: vec![End::NotRun],
            }
        );
        let restore = sent.last().unwrap();
        assert_eq!(restore.if_match.as_deref(), Some("\"m2\""));
        assert!(restore.url.contains("sendUpdates=none"));
        let body = restore.body.as_ref().unwrap();
        assert_eq!(body["recurrence"], old["recurrence"]);
        assert_eq!(body["start"], old["start"]);
    }

    /// The put-back is proved only by Google holding the old times and rule again: text sent with
    /// the move may stay. Anything else after the restore is unknown.
    #[test]
    fn a_restore_is_proved_only_by_the_old_times_and_rule() {
        let case = Case::new(weekly(), monday_19());
        let mut draft = at("09:00", "2026-10-20");
        draft.summary = Some("Standup v2".into());
        let plan = case.edit(Scope::All, draft).unwrap();
        let old = case.master.json().clone();
        let mut half = landed(&old, &plan.steps[0], "\"m2\"");
        half["recurrence"] = old["recurrence"].clone();
        let text_kept = at_etag(&with(old.clone(), "summary", json!("Standup v2")), "\"m3\"");
        let run_with = |restore: Answer| {
            let (half, plan) = (half.clone(), plan.clone());
            drive(&mut Run::new(plan).unwrap(), move |r| match r.method {
                Method::Patch if r.if_match.as_deref() == Some("\"m1\"") => ok(half.clone()),
                Method::Patch => restore.clone(),
                _ => ok(half.clone()),
            })
            .0
        };
        assert!(matches!(
            run_with(ok(text_kept)),
            Ran::NotDone {
                why: Why::PutBack,
                ..
            }
        ));
        for restore in [
            ok(at_etag(&half, "\"m3\"")),
            status(Verdict::Conflict),
            lost(),
            Answer::Reply {
                verdict: Verdict::Ok,
                body: None,
                date: None,
                maybe_applied: false,
            },
        ] {
            assert!(
                matches!(run_with(restore.clone()), Ran::Unknown { step: 0, .. }),
                "{restore:?}"
            );
        }
    }

    /// Verify-pass finding: every copy of the move was refused with a 412, and someone else has
    /// half-moved the series since. That half is theirs, never PM's to put back: a conflict, and no
    /// restore is written over their change.
    #[test]
    fn a_half_move_no_copy_made_is_a_conflict_never_put_back() {
        let (case, plan) = all_to_tuesday();
        let old = case.master.json().clone();
        let mut theirs = landed(&old, &plan.steps[0], "\"m5\"");
        theirs["recurrence"] = old["recurrence"].clone();
        let (ran, sent, _) = drive(&mut Run::new(plan).unwrap(), |r| match r.method {
            Method::Patch => status(Verdict::Conflict),
            _ => ok(theirs.clone()),
        });
        let Ran::NotDone {
            why: Why::Conflict(fields),
            rollback: Rollback::Nothing,
            ..
        } = &ran
        else {
            panic!("{ran:?}");
        };
        assert!(fields.iter().any(|f| f == "start"), "{fields:?}");
        assert_eq!(sent.iter().filter(|r| r.method == Method::Patch).count(), 1);
    }

    #[test]
    fn never_restore_master_on_a_plain_unknown() {
        let (case, plan) = all_to_tuesday();
        let elsewhere = at_etag(
            &with(
                case.master.json().clone(),
                "recurrence",
                json!(["RRULE:FREQ=DAILY"]),
            ),
            "\"m2\"",
        );
        let (ran, sent, _) = drive(&mut Run::new(plan).unwrap(), |_| ok(elsewhere.clone()));
        assert!(matches!(ran, Ran::Unknown { step: 0, .. }));
        assert_eq!(sent.iter().filter(|r| r.method == Method::Patch).count(), 1);
    }

    // --- Run: carries ---

    #[test]
    fn a_carry_is_looked_up_again_after_1s_then_3s_then_skipped_with_its_cancel() {
        let (case, plan) = split_case(vec![renamed()]);
        let tail = tail_of(&plan, "\"t1\"");
        let cut = landed(case.master.json(), &plan.steps[1], "\"m2\"");
        let (ran, sent, waits) = drive(&mut Run::new(plan).unwrap(), |r| match r.method {
            Method::Insert => ok(tail.clone()),
            Method::Patch => ok(cut.clone()),
            Method::Get => ok(json!({ "items": [] })),
            Method::Delete => deleted(),
        });
        assert_eq!(waits, LOOK_AGAIN.to_vec());
        assert_eq!(sent.iter().filter(|r| r.method == Method::Get).count(), 3);
        assert_eq!(ran.skipped(), vec![2, 3]);
        assert!(matches!(ran, Ran::Done { .. }));
        let look = &sent[2];
        assert!(look
            .url
            .contains(&format!("/events/{TAIL}/instances?showDeleted=true")));
        assert!(look.url.contains("timeMin=2026-11-01T09%3A00%3A00Z"));
        assert!(look.url.contains("timeMax=2026-11-03T09%3A00%3A00Z"));
    }

    #[test]
    fn a_carry_matches_by_original_start_never_by_id() {
        let (case, plan) = split_case(vec![renamed()]);
        let tail = tail_of(&plan, "\"t1\"");
        let cut = landed(case.master.json(), &plan.steps[1], "\"m2\"");
        let items = json!({ "items": [
            { "id": "zzz", "etag": "\"z\"", "status": "cancelled",
              "originalStartTime": { "dateTime": "2026-11-02T09:00:00Z" } },
            { "id": format!("{TAIL}_20261102T090000Z"), "etag": "\"w\"", "status": "confirmed",
              "originalStartTime": { "dateTime": "2026-11-09T09:00:00Z" } },
            { "id": "q1", "etag": "\"q\"", "status": "confirmed",
              "originalStartTime": { "dateTime": "2026-11-02T09:00:00Z" } },
        ]});
        let (_, sent, _) = drive(&mut Run::new(plan).unwrap(), |r| match r.method {
            Method::Insert => ok(tail.clone()),
            Method::Patch if to(r, "abc") => ok(cut.clone()),
            Method::Get => ok(items.clone()),
            _ => lost(),
        });
        let carry = sent
            .iter()
            .find(|r| r.method == Method::Patch && !to(r, "abc"))
            .unwrap();
        assert!(to(carry, "q1"));
        assert_eq!(carry.if_match.as_deref(), Some("\"q\""));
    }

    #[test]
    fn a_cleanup_step_unknown_is_skipped_with_what_requires_it() {
        let (case, plan) = split_case(vec![renamed()]);
        let tail = tail_of(&plan, "\"t1\"");
        let cut = landed(case.master.json(), &plan.steps[1], "\"m2\"");
        let occurrence = json!({ "items": [{ "id": "q1", "etag": "\"q\"", "status": "confirmed",
            "originalStartTime": { "dateTime": "2026-11-02T09:00:00Z" } }] });
        let (ran, sent, _) = drive(&mut Run::new(plan).unwrap(), |r| match r.method {
            Method::Insert => ok(tail.clone()),
            Method::Patch if to(r, "abc") => ok(cut.clone()),
            Method::Get if r.url.contains("/instances") => ok(occurrence.clone()),
            _ => lost(),
        });
        assert_eq!(ran.skipped(), vec![2, 3]);
        assert!(matches!(ran, Ran::Done { .. }));
        // The cancel that required the carry was never sent.
        assert!(!sent.iter().any(|r| to(r, "abc_20261102T090000Z")));
    }

    #[test]
    fn a_split_that_goes_through_lands_every_step() {
        let (case, plan) = split_case(vec![renamed()]);
        let tail = tail_of(&plan, "\"t1\"");
        let (ran, _, _) = drive(&mut Run::new(plan.clone()).unwrap(), |r| {
            finished_split(&case, &plan, &tail, r)
        });
        let Ran::Done { ends } = &ran else {
            panic!("{ran:?}");
        };
        assert!(
            ends.iter().all(|e| matches!(e, End::Done { .. })),
            "{ends:?}"
        );
        assert_eq!(ran.skipped(), Vec::<usize>::new());
    }

    #[test]
    fn a_run_starts_only_where_every_commit_step_before_it_landed() {
        let (_, plan) = split_case(Vec::new());
        let tail = tail_of(&plan, "\"t1\"");
        let done = |reply: Option<Value>| End::Done { reply, date: None };
        for ends in [
            vec![End::NotRun],
            vec![End::Undone, End::NotRun],
            vec![End::NotRun, done(None)],
            // A cut after an insert that never landed, or one with no copy to undo it on.
            vec![End::Skipped, End::NotRun],
            vec![done(None), End::NotRun],
        ] {
            assert_eq!(
                Run::from(plan.clone(), ends.clone()).err(),
                Some(Unfit),
                "{ends:?}"
            );
        }
        assert!(Run::from(plan, vec![done(Some(tail)), End::NotRun]).is_ok());
    }

    /// Finish needs the proof Settle found, paired with the run it settles: never a cut sent again
    /// on an unknown, a conflict, or a proof of something else.
    #[test]
    fn finish_takes_only_settles_proof() {
        let (case, plan, unknown_cut) = partial(weekly());
        let tail = tail_of(&plan, "\"t1\"");
        let cut = landed(case.master.json(), &plan.steps[1], "\"m2\"");
        let cut_done = Settled::CutDone {
            original: cut.clone(),
        };
        let tail_there = Settled::TailThere { tail: tail.clone() };
        assert!(Run::finish(plan.clone(), &unknown_cut, &cut_done).is_ok());
        for settled in [
            tail_there.clone(),
            // A copy that doesn't show what it's offered as: the original uncut, a deleted tail.
            Settled::CutDone {
                original: case.master.json().clone(),
            },
            Settled::CutUnproved,
            Settled::CutNotDone,
            Settled::Unknown,
            Settled::Removed,
        ] {
            assert_eq!(
                Run::finish(plan.clone(), &unknown_cut, &settled).err(),
                Some(Unfit),
                "{settled:?}"
            );
        }
        let (unknown_insert, _, _) = drive(&mut Run::new(plan.clone()).unwrap(), |_| lost());
        assert!(Run::finish(plan.clone(), &unknown_insert, &tail_there).is_ok());
        assert_eq!(
            Run::finish(plan.clone(), &unknown_insert, &cut_done).err(),
            Some(Unfit)
        );
        let deleted_tail = Settled::TailThere {
            tail: with(tail.clone(), "status", json!("cancelled")),
        };
        assert_eq!(
            Run::finish(plan.clone(), &unknown_insert, &deleted_tail).err(),
            Some(Unfit)
        );
        let (refused, _, _) = drive(&mut Run::new(plan.clone()).unwrap(), |r| match r.method {
            Method::Insert => ok(tail.clone()),
            Method::Patch => status(Verdict::ReadOnly),
            _ => status(Verdict::ReadOnly),
        });
        assert!(matches!(
            refused,
            Ran::NotDone {
                rollback: Rollback::Unknown { step: 0 },
                ..
            }
        ));
        assert_eq!(Run::finish(plan, &refused, &cut_done).err(), Some(Unfit));
    }

    /// After an unknown insert whose new series Settle found, Finish sends the cut onward.
    #[test]
    fn finish_after_an_unknown_insert_sends_the_cut_onward() {
        let (case, plan) = split_case(Vec::new());
        let tail = tail_of(&plan, "\"t1\"");
        let cut = landed(case.master.json(), &plan.steps[1], "\"m2\"");
        let (ran, _, _) = drive(&mut Run::new(plan.clone()).unwrap(), |_| lost());
        let settled = Settled::TailThere { tail: tail.clone() };
        let (ran, sent, _) = drive(&mut Run::finish(plan, &ran, &settled).unwrap(), |_| {
            ok(cut.clone())
        });
        assert!(matches!(ran, Ran::Done { .. }));
        assert_eq!(sent.len(), 1);
        assert!(to(&sent[0], "abc") && sent[0].method == Method::Patch);
    }

    // --- Settle ---

    /// A split whose cut was sent once on `"m1"` and never answered.
    fn partial(case_master: Value) -> (Case, SeriesPlan, Ran) {
        let case = Case::new(case_master, monday_19());
        let plan = case.edit(Scope::Following, title("Standup v2")).unwrap();
        let tail = tail_of(&plan, "\"t1\"");
        let (ran, _, _) = drive(&mut Run::new(plan.clone()).unwrap(), |r| match r.method {
            Method::Insert => ok(tail.clone()),
            _ => Answer::Lost { retried: true },
        });
        assert!(matches!(ran, Ran::Unknown { step: 1, .. }), "{ran:?}");
        (case, plan, ran)
    }

    /// Never re-send the cut to settle it (plan.rs).
    fn sends_the_cut(sent: &[RequestPlan]) -> bool {
        sent.iter().any(|r| {
            r.method == Method::Patch
                && r.body
                    .as_ref()
                    .is_some_and(|b| b.get("recurrence").is_some())
        })
    }

    #[test]
    fn settle_takes_only_a_partial_split() {
        let (_, plan) = this_case();
        let done = Ran::Done {
            ends: vec![End::NotRun],
        };
        assert!(Settle::new(plan, &done, SettleAction::Check).is_none());
        let (_, plan, _) = partial(weekly());
        assert!(Settle::new(plan, &done, SettleAction::Remove).is_none());
    }

    #[test]
    fn remove_at_a_moved_etag_deletes_the_new_series_without_a_fence() {
        let (case, plan, ran) = partial(weekly());
        let tail = tail_of(&plan, "\"t1\"");
        let mut settle = Settle::new(plan, &ran, SettleAction::Remove).unwrap();
        let (settled, sent, _) = drive(&mut settle, |r| match r.method {
            Method::Get if to(r, "abc") => ok(at_etag(case.master.json(), "\"m7\"")),
            Method::Get => ok(tail.clone()),
            Method::Delete => deleted(),
            _ => panic!("no write but the delete: {r:?}"),
        });
        assert_eq!(settled, Settled::Removed);
        assert!(!sends_the_cut(&sent));
        assert_eq!(sent.last().unwrap().if_match.as_deref(), Some("\"t1\""));
    }

    /// At the cut's own etag a copy of it could still land: the fence write moves it first, keeping
    /// every property anyone else put there.
    #[test]
    fn remove_at_the_cut_etag_fences_first() {
        let props = json!({ "private": { "theirs": "1" }, "shared": { "app": "x" } });
        let (case, plan, ran) = partial(with(weekly(), "extendedProperties", props));
        let tail = tail_of(&plan, "\"t1\"");
        let old = case.master.json().clone();
        let mut settle = Settle::new(plan.clone(), &ran, SettleAction::Remove).unwrap();
        let (settled, sent, _) = drive(&mut settle, |r| match r.method {
            Method::Get if to(r, "abc") => ok(old.clone()),
            Method::Patch => ok(at_etag(&old, "\"m8\"")),
            Method::Get => ok(tail.clone()),
            Method::Delete => deleted(),
            Method::Insert => panic!("never an insert"),
        });
        assert_eq!(settled, Settled::Removed);
        assert!(!sends_the_cut(&sent));
        let fence = sent.iter().find(|r| r.method == Method::Patch).unwrap();
        assert_eq!(fence.if_match.as_deref(), Some("\"m1\""));
        assert!(fence.url.contains("sendUpdates=none"));
        let Shows::Tail { plan: digest, .. } = &plan.steps[0].shows else {
            panic!("a split inserts first");
        };
        assert_eq!(
            fence.body,
            Some(json!({ "extendedProperties": {
                "private": { "theirs": "1", "pmFence": format!("{digest} \"m1\"") },
                "shared": { "app": "x" },
            }}))
        );
    }

    /// Review finding: a fence answered 2xx that left Google at the cut's etag (it changed nothing)
    /// proves nothing; look again, and only an etag past every copy of the cut proves it.
    #[test]
    fn a_fence_that_didnt_move_the_etag_proves_nothing() {
        let (case, plan, ran) = partial(weekly());
        let tail = tail_of(&plan, "\"t1\"");
        let old = case.master.json().clone();
        let mut settle = Settle::new(plan.clone(), &ran, SettleAction::Remove).unwrap();
        let (settled, sent, _) = drive(&mut settle, |r| match r.method {
            // The new series is there to delete: only the fence's etag can stop it.
            Method::Get if to(r, TAIL) => ok(tail.clone()),
            Method::Get | Method::Patch => ok(old.clone()),
            _ => panic!("nothing else: {r:?}"),
        });
        assert_eq!(settled, Settled::Unknown);
        let methods: Vec<Method> = sent.iter().map(|r| r.method).collect();
        assert_eq!(methods, [Method::Get, Method::Patch, Method::Get]);
        assert!(to(&sent[0], "abc") && to(&sent[2], "abc"));

        // Looked at again, Google is past every copy of the cut: proved, and the new series goes.
        let mut gets = 0;
        let mut settle = Settle::new(plan, &ran, SettleAction::Remove).unwrap();
        let (settled, sent, _) = drive(&mut settle, |r| match r.method {
            Method::Get if to(r, TAIL) => ok(tail.clone()),
            Method::Get => {
                gets += 1;
                ok(if gets == 1 {
                    old.clone()
                } else {
                    at_etag(&old, "\"m9\"")
                })
            }
            Method::Patch => ok(old.clone()),
            Method::Delete => deleted(),
            Method::Insert => panic!("never an insert"),
        });
        assert_eq!(settled, Settled::Removed);
        let last = sent.last().unwrap();
        assert!(last.method == Method::Delete && to(last, TAIL));
        assert_eq!(last.if_match.as_deref(), Some("\"t1\""));
    }

    /// Review finding: a copy of the original without an etag can't say which etag Google is at.
    #[test]
    fn an_original_without_an_etag_proves_nothing() {
        let (case, plan, ran) = partial(weekly());
        let mut no_etag = case.master.json().clone();
        no_etag.as_object_mut().unwrap().remove("etag");
        for action in [SettleAction::Check, SettleAction::Remove] {
            let mut settle = Settle::new(plan.clone(), &ran, action).unwrap();
            let (settled, sent, _) = drive(&mut settle, |_| ok(no_etag.clone()));
            assert_eq!(settled, Settled::Unknown);
            assert_eq!(sent.len(), 1);
        }
    }

    /// A cut proved not done whose rollback couldn't delete the new series (Cut::Proved).
    fn refused_with_rollback_unknown() -> (SeriesPlan, Value, Ran) {
        let (_, plan) = split_case(Vec::new());
        let tail = tail_of(&plan, "\"t1\"");
        let (ran, _, _) = drive(&mut Run::new(plan.clone()).unwrap(), |r| match r.method {
            Method::Insert => ok(tail.clone()),
            _ => status(Verdict::ReadOnly),
        });
        assert!(matches!(
            ran,
            Ran::NotDone {
                rollback: Rollback::Unknown { step: 0 },
                ..
            }
        ));
        (plan, tail, ran)
    }

    #[test]
    fn a_proved_cut_whose_new_series_stayed_is_removed_by_the_undos_rules() {
        let (plan, tail, ran) = refused_with_rollback_unknown();
        let mut settle = Settle::new(plan.clone(), &ran, SettleAction::Check).unwrap();
        let (settled, sent, _) = drive(&mut settle, |_| ok(tail.clone()));
        assert_eq!(settled, Settled::CutNotDone);
        assert!(sent.iter().all(|r| r.method == Method::Get));
        let mut settle = Settle::new(plan.clone(), &ran, SettleAction::Remove).unwrap();
        let (settled, sent, _) = drive(&mut settle, |r| match r.method {
            Method::Delete => deleted(),
            _ => ok(tail.clone()),
        });
        assert_eq!(settled, Settled::Removed);
        assert_eq!(sent.last().unwrap().if_match.as_deref(), Some("\"t1\""));
        assert!(!sends_the_cut(&sent));
    }

    /// Review finding: Google keeps a deleted event as a cancelled copy, so the rollback's delete
    /// that landed after all reads as removed, never as unknown for good.
    #[test]
    fn a_deleted_new_series_settles_whatever_the_cut() {
        let (plan, tail, ran) = refused_with_rollback_unknown();
        let cancelled = with(tail.clone(), "status", json!("cancelled"));
        for action in [SettleAction::Check, SettleAction::Remove] {
            for google in [ok(cancelled.clone()), status(Verdict::Gone)] {
                let mut settle = Settle::new(plan.clone(), &ran, action).unwrap();
                let (settled, sent, waits) = drive(&mut settle, |_| google.clone());
                assert_eq!(settled, Settled::Removed, "{action:?} {google:?}");
                assert_eq!(sent.len(), 1);
                assert!(waits.is_empty());
            }
        }
        let (unknown_insert, _, _) = drive(&mut Run::new(plan.clone()).unwrap(), |_| lost());
        for (action, want) in [
            (SettleAction::Check, Settled::TailAbsent),
            (SettleAction::Remove, Settled::Removed),
        ] {
            let mut settle = Settle::new(plan.clone(), &unknown_insert, action).unwrap();
            let (settled, sent, _) = drive(&mut settle, |_| ok(cancelled.clone()));
            assert_eq!(settled, want);
            assert_eq!(sent.len(), 1);
        }
    }

    #[test]
    fn settle_takes_only_a_run_of_this_plan() {
        let (_, plan, ran) = partial(weekly());
        let Ran::Unknown { step, sent_on, .. } = ran else {
            panic!("partial");
        };
        let short = Ran::Unknown {
            step,
            sent_on,
            ends: Vec::new(),
        };
        assert!(Settle::new(plan, &short, SettleAction::Check).is_none());
    }

    /// L5: an all-day series' occurrence is looked for with instants, a day either side of its date.
    #[test]
    fn an_all_day_carry_is_looked_for_with_instants() {
        let day = chrono::NaiveDate::from_ymd_opt(2026, 11, 2).unwrap();
        let look = carry_lookup("c", TAIL, Point::Day(day));
        assert!(
            look.url.contains("timeMin=2026-11-01T00%3A00%3A00Z"),
            "{}",
            look.url
        );
        assert!(
            look.url.contains("timeMax=2026-11-04T00%3A00%3A00Z"),
            "{}",
            look.url
        );
        let occurrence = json!({ "items": [{ "id": "d1", "etag": "\"d\"", "status": "confirmed",
            "originalStartTime": { "date": "2026-11-02" } }] });
        assert_eq!(
            find_occurrence(&occurrence, Point::Day(day)),
            Some(("d1".into(), "\"d\"".into()))
        );
    }

    #[test]
    fn a_fence_412_takes_the_cuts_verdict() {
        let (case, plan, ran) = partial(weekly());
        let tail = tail_of(&plan, "\"t1\"");
        let old = case.master.json().clone();
        let cut = landed(&old, &plan.steps[1], "\"m9\"");
        let mut gets = 0;
        let mut settle = Settle::new(plan.clone(), &ran, SettleAction::Remove).unwrap();
        let (settled, sent, _) = drive(&mut settle, |r| match r.method {
            Method::Get => {
                gets += 1;
                ok(if gets == 1 { old.clone() } else { cut.clone() })
            }
            Method::Patch => status(Verdict::Conflict),
            _ => panic!("nothing else: {r:?}"),
        });
        assert_eq!(settled, Settled::CutDone { original: cut });
        assert!(!sent.iter().any(|r| r.method == Method::Delete));

        // The etag moved, and not by the cut: proved, and the new series goes.
        let mut gets = 0;
        let mut settle = Settle::new(plan, &ran, SettleAction::Remove).unwrap();
        let (settled, _, _) = drive(&mut settle, |r| match r.method {
            Method::Get if to(r, "abc") => {
                gets += 1;
                ok(if gets == 1 {
                    old.clone()
                } else {
                    at_etag(&old, "\"m9\"")
                })
            }
            Method::Get => ok(tail.clone()),
            Method::Patch => status(Verdict::Conflict),
            Method::Delete => deleted(),
            Method::Insert => panic!("never an insert"),
        });
        assert_eq!(settled, Settled::Removed);
    }

    /// The cut landed: Remove would delete the only copy of every occurrence from the split on.
    #[test]
    fn a_done_cut_offers_finish_never_remove() {
        let (case, plan, ran) = partial(weekly());
        let cut = landed(case.master.json(), &plan.steps[1], "\"m2\"");
        let mut settle = Settle::new(plan, &ran, SettleAction::Remove).unwrap();
        let (settled, sent, _) = drive(&mut settle, |_| ok(cut.clone()));
        assert_eq!(settled, Settled::CutDone { original: cut });
        assert_eq!(sent.len(), 1);
    }

    #[test]
    fn a_cut_pm_cant_read_settles_nothing() {
        let (case, plan, ran) = partial(weekly());
        let elsewhere = at_etag(
            &with(
                case.master.json().clone(),
                "recurrence",
                json!(["RRULE:FREQ=DAILY"]),
            ),
            "\"m2\"",
        );
        for action in [SettleAction::Check, SettleAction::Remove] {
            let mut settle = Settle::new(plan.clone(), &ran, action).unwrap();
            let (settled, sent, _) = drive(&mut settle, |_| ok(elsewhere.clone()));
            assert_eq!(settled, Settled::Unknown);
            assert_eq!(sent.len(), 1);
        }
    }

    #[test]
    fn check_only_looks() {
        let (case, plan, ran) = partial(weekly());
        let old = case.master.json().clone();
        let cut = landed(&old, &plan.steps[1], "\"m2\"");
        for (google, want) in [
            (at_etag(&old, "\"m7\""), Settled::CutNotDone),
            (old.clone(), Settled::CutUnproved),
            (
                cut.clone(),
                Settled::CutDone {
                    original: cut.clone(),
                },
            ),
        ] {
            let mut settle = Settle::new(plan.clone(), &ran, SettleAction::Check).unwrap();
            let (settled, sent, _) = drive(&mut settle, |_| ok(google.clone()));
            assert_eq!(settled, want);
            assert!(sent.iter().all(|r| r.method == Method::Get));
        }
    }

    #[test]
    fn an_absent_new_series_is_looked_for_at_1s_and_3s() {
        let (_, plan) = split_case(Vec::new());
        let (ran, _, _) = drive(&mut Run::new(plan.clone()).unwrap(), |_| lost());
        assert!(matches!(ran, Ran::Unknown { step: 0, .. }));
        for (action, want) in [
            (SettleAction::Check, Settled::TailAbsent),
            (SettleAction::Remove, Settled::Removed),
        ] {
            let mut settle = Settle::new(plan.clone(), &ran, action).unwrap();
            let (settled, sent, waits) = drive(&mut settle, |_| status(Verdict::Gone));
            assert_eq!(settled, want);
            assert_eq!(waits, LOOK_AGAIN.to_vec());
            assert!(sent.iter().all(|r| r.method == Method::Get && to(r, TAIL)));
        }
        let tail = tail_of(&plan, "\"t1\"");
        let mut settle = Settle::new(plan.clone(), &ran, SettleAction::Check).unwrap();
        let (settled, _, _) = drive(&mut settle, |_| ok(tail.clone()));
        assert_eq!(settled, Settled::TailThere { tail: tail.clone() });
        let mut settle = Settle::new(plan, &ran, SettleAction::Remove).unwrap();
        let (settled, sent, _) = drive(&mut settle, |r| match r.method {
            Method::Delete => deleted(),
            _ => ok(tail.clone()),
        });
        assert_eq!(settled, Settled::Removed);
        assert!(!sends_the_cut(&sent));
    }

    /// "Finish": the rest of the same plan after a cut proved done, never the insert or the cut again.
    #[test]
    fn finish_resumes_the_same_plan_after_the_cut() {
        let (case, plan) = split_case(vec![renamed()]);
        let tail = tail_of(&plan, "\"t1\"");
        let cut = landed(case.master.json(), &plan.steps[1], "\"m2\"");
        let (unknown_cut, _, _) = drive(&mut Run::new(plan.clone()).unwrap(), |r| match r.method {
            Method::Insert => ok(tail.clone()),
            _ => Answer::Lost { retried: true },
        });
        assert!(matches!(unknown_cut, Ran::Unknown { step: 1, .. }));
        let settled = Settled::CutDone { original: cut };
        let (ran, sent, _) = drive(
            &mut Run::finish(plan.clone(), &unknown_cut, &settled).unwrap(),
            |r| finished_split(&case, &plan, &tail, r),
        );
        let Ran::Done { ends } = &ran else {
            panic!("{ran:?}");
        };
        assert!(
            ends.iter().all(|e| matches!(e, End::Done { .. })),
            "{ends:?}"
        );
        assert!(ran.skipped().is_empty());
        assert!(sent[0].url.contains("/instances"));
        assert!(!sent
            .iter()
            .any(|r| r.method == Method::Insert || to(r, "abc")));
    }

    #[test]
    fn the_fence_needs_an_etag_and_a_plain_id() {
        assert!(fence_request("c", &json!({ "id": "abc" }), "p").is_none());
        assert!(fence_request("c", &json!({ "id": "..", "etag": "\"1\"" }), "p").is_none());
        let fence = fence_request("c", &json!({ "id": "abc", "etag": "\"1\"" }), "p").unwrap();
        assert_eq!(
            fence.body,
            Some(json!({ "extendedProperties": { "private": { "pmFence": "p" } } }))
        );
    }
}
