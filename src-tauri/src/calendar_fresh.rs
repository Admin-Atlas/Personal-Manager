// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Keeping Google calendars fresh between full syncs (#884, F3; Bobby: "google change, pm needs me to
//! press refresh … it should be maybe 30 seconds or at most a minute").
//!
//! A full sync re-downloads every selected calendar's whole mirrored band, every iCal feed and every
//! account's calendar list, so it runs every 15 minutes. In between, while PM's window is on screen,
//! the webview asks about every 30 seconds for a cheap check: one small request per Google calendar,
//! "which events changed since H?" (`calendar::check_for_changes`: ids and timestamps only), and only a
//! calendar that answers with something new is fetched again.
//!
//! H, the high-water mark, is read off Google's own clock (the `Date` header of a reply), never this
//! computer's: the laptop's clock can run hours fast, and an `updatedMin` in Google's future would hide
//! every change for as long as it ran ahead. H is set back by [`LAG`] to cover the moment between a
//! change and its showing in a listing; what that overlap lists again is recognised by its
//! (id, updated) and isn't counted as new.
//!
//! This module is the bookkeeping only, pure apart from the `Instant`s it is handed, so it tests
//! without a network or a clock; `commands::calendars` does the I/O around it.

use chrono::{DateTime, TimeDelta, Utc};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// How far behind Google's clock H is kept: a change can take a moment to show in a listing.
pub fn lag() -> TimeDelta {
    TimeDelta::seconds(60)
}

/// The shortest gap between two checks, however often one is asked for (the tick, the window coming
/// back into view, a focus).
pub const MIN_GAP: Duration = Duration::from_secs(10);

/// After a failed check or fetch: wait 30 s, doubling each time, to at most 15 minutes (the full
/// sync's own cadence), as Google asks of a client that keeps failing.
const BACKOFF_BASE: Duration = Duration::from_secs(30);
const BACKOFF_MAX: Duration = Duration::from_secs(15 * 60);

/// What PM knows about keeping each Google calendar fresh. In memory only: after a restart the first
/// full sync sets it up again.
#[derive(Default)]
pub struct Freshness {
    calendars: HashMap<String, CalendarFresh>,
    last_check: Option<Instant>,
}

#[derive(Default)]
struct CalendarFresh {
    /// The `updatedMin` of the next check, by Google's clock. `None` until a complete fetch has read
    /// Google's clock for this calendar.
    since: Option<DateTime<Utc>>,
    /// Changes already fetched, as (event id, updated): the [`lag`] overlap lists them again.
    absorbed: HashSet<(String, String)>,
    failures: u32,
    retry_at: Option<Instant>,
    /// Google wants the account signed in again: no checks until a full sync gets through.
    needs_sign_in: bool,
    /// When a fetch last ran without giving PM Google's clock (no `Date` header): not again before
    /// the backoff's longest wait, so a reply missing it can't turn every check into a fetch.
    blind_fetch: Option<Instant>,
}

/// What to do about one calendar now.
#[derive(Debug, PartialEq)]
pub enum Plan {
    /// Leave it: it's waiting out a failure, or its account needs signing in again.
    Skip,
    /// Fetch it: nothing yet says from when to check.
    Fetch,
    /// Ask Google what changed since this time.
    Check(DateTime<Utc>),
}

/// What a check found.
#[derive(Debug, PartialEq)]
pub enum Verdict {
    Unchanged,
    Changed,
}

impl Freshness {
    /// Whether a check may run now: none ran in the last [`MIN_GAP`]. Notes this one as run.
    pub fn begin_check(&mut self, now: Instant) -> bool {
        if self
            .last_check
            .is_some_and(|last| now.saturating_duration_since(last) < MIN_GAP)
        {
            return false;
        }
        self.last_check = Some(now);
        true
    }

    /// Forget every calendar not in `selected` (unticked, disconnected).
    pub fn keep_only(&mut self, selected: &HashSet<&str>) {
        self.calendars
            .retain(|id, _| selected.contains(id.as_str()));
    }

    /// What to do about `calendar_id` now.
    pub fn plan(&self, calendar_id: &str, now: Instant) -> Plan {
        let Some(c) = self.calendars.get(calendar_id) else {
            return Plan::Fetch;
        };
        if c.needs_sign_in || c.retry_at.is_some_and(|at| now < at) {
            return Plan::Skip;
        }
        match c.since {
            Some(since) => Plan::Check(since),
            None if c
                .blind_fetch
                .is_some_and(|at| now.saturating_duration_since(at) < BACKOFF_MAX) =>
            {
                Plan::Skip
            }
            None => Plan::Fetch,
        }
    }

    /// A check's answer: the (id, updated) pairs it listed, whether there were more than it read, and
    /// Google's clock. Nothing new: H moves up to Google's time (less the [`lag`]), and a run of
    /// failures is over. Something new: the fetch that follows decides that ([`Self::fetched`] ends
    /// the run, [`Self::failed`] extends it), so a calendar whose fetch keeps failing still backs off.
    pub fn checked(
        &mut self,
        calendar_id: &str,
        items: &[(String, String)],
        more: bool,
        server_time: Option<DateTime<Utc>>,
    ) -> Verdict {
        let c = self.calendars.entry(calendar_id.to_string()).or_default();
        if more || items.iter().any(|item| !c.absorbed.contains(item)) {
            return Verdict::Changed;
        }
        c.failures = 0;
        c.retry_at = None;
        if let Some(t) = server_time {
            let next = t - lag();
            if c.since.is_none_or(|since| next > since) {
                c.since = Some(next);
            }
        }
        c.prune();
        Verdict::Unchanged
    }

    /// A complete fetch landed. H becomes Google's time when it answered the fetch's first page (less
    /// the [`lag`]), the changes the check that asked for it listed are absorbed, and any failure or
    /// sign-in pause is over.
    pub fn fetched(
        &mut self,
        calendar_id: &str,
        server_time: Option<DateTime<Utc>>,
        absorb: &[(String, String)],
        now: Instant,
    ) {
        let c = self.calendars.entry(calendar_id.to_string()).or_default();
        c.failures = 0;
        c.retry_at = None;
        c.needs_sign_in = false;
        c.absorbed.extend(absorb.iter().cloned());
        match server_time {
            Some(t) => {
                c.since = Some(t - lag());
                c.blind_fetch = None;
            }
            None => c.blind_fetch = Some(now),
        }
        c.prune();
    }

    /// Google refused a check or a fetch, or answered it wrongly: wait before the next try (doubling
    /// each time), and stop altogether while the account needs signing in again.
    pub fn failed(&mut self, calendar_id: &str, now: Instant, needs_sign_in: bool) {
        let c = self.calendars.entry(calendar_id.to_string()).or_default();
        c.failures = c.failures.saturating_add(1);
        c.retry_at = Some(now + backoff(c.failures));
        c.needs_sign_in |= needs_sign_in;
    }

    /// A check or fetch never reached Google (offline, no DNS, a connection that timed out): try again
    /// with the next check, without counting it. Google's back-off is for its own refusals; counting
    /// a dropped hotspot would leave PM up to 15 minutes behind once the connection came back.
    pub fn unreachable(&mut self, calendar_id: &str, now: Instant) {
        let c = self.calendars.entry(calendar_id.to_string()).or_default();
        c.retry_at = Some(now + MIN_GAP);
    }
}

impl CalendarFresh {
    /// Drop absorbed changes older than H: a check from H on can never list them again.
    fn prune(&mut self) {
        let Some(since) = self.since else {
            return;
        };
        self.absorbed.retain(|(_, updated)| {
            DateTime::parse_from_rfc3339(updated)
                .map(|u| u.with_timezone(&Utc) >= since)
                .unwrap_or(true)
        });
    }
}

/// The wait after `failures` failures in a row: 30 s, 1 min, 2 min … at most 15 min.
pub fn backoff(failures: u32) -> Duration {
    let doublings = failures.saturating_sub(1).min(16);
    BACKOFF_BASE
        .saturating_mul(1u32 << doublings)
        .min(BACKOFF_MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    fn item(id: &str, updated: &str) -> (String, String) {
        (id.to_string(), updated.to_string())
    }

    /// A calendar PM has never fetched is fetched; once a fetch has read Google's clock, it is
    /// checked from that time less the lag, never from this computer's clock.
    #[test]
    fn a_calendar_is_checked_from_googles_clock_after_its_first_fetch() {
        let now = Instant::now();
        let mut f = Freshness::default();
        assert_eq!(f.plan("cal", now), Plan::Fetch);
        f.fetched("cal", Some(t("2026-10-10T10:00:00Z")), &[], now);
        assert_eq!(f.plan("cal", now), Plan::Check(t("2026-10-10T09:59:00Z")));
    }

    /// Nothing listed, or only what was already fetched: unchanged, and H moves up with Google's
    /// clock. Anything new, or more than one page: changed, and H waits for the fetch.
    #[test]
    fn only_a_change_not_yet_fetched_counts() {
        let now = Instant::now();
        let mut f = Freshness::default();
        f.fetched(
            "cal",
            Some(t("2026-10-10T10:00:00Z")),
            &[item("a", "2026-10-10T09:59:30Z")],
            now,
        );
        // The overlap lists the change the fetch already took: not new.
        let seen = [item("a", "2026-10-10T09:59:30Z")];
        assert_eq!(
            f.checked("cal", &seen, false, Some(t("2026-10-10T10:00:30Z"))),
            Verdict::Unchanged
        );
        assert_eq!(f.plan("cal", now), Plan::Check(t("2026-10-10T09:59:30Z")));
        // The same event changed again: new.
        let again = [item("a", "2026-10-10T10:00:10Z")];
        assert_eq!(
            f.checked("cal", &again, false, Some(t("2026-10-10T10:01:00Z"))),
            Verdict::Changed
        );
        // A changed check leaves H where it was until the fetch lands.
        assert_eq!(f.plan("cal", now), Plan::Check(t("2026-10-10T09:59:30Z")));
        assert_eq!(f.checked("cal", &[], true, None), Verdict::Changed);
        assert_eq!(f.checked("cal", &[], false, None), Verdict::Unchanged);
    }

    /// H never goes back, whatever a reply's clock says.
    #[test]
    fn the_high_water_mark_only_moves_forward() {
        let now = Instant::now();
        let mut f = Freshness::default();
        f.fetched("cal", Some(t("2026-10-10T10:00:00Z")), &[], now);
        f.checked("cal", &[], false, Some(t("2026-10-10T09:00:00Z")));
        assert_eq!(f.plan("cal", now), Plan::Check(t("2026-10-10T09:59:00Z")));
    }

    /// What the fetch absorbed is forgotten once H has passed it, so the set stays small.
    #[test]
    fn absorbed_changes_older_than_the_mark_are_dropped() {
        let now = Instant::now();
        let mut f = Freshness::default();
        f.fetched(
            "cal",
            Some(t("2026-10-10T10:00:00Z")),
            &[item("a", "2026-10-10T09:59:30Z")],
            now,
        );
        f.checked("cal", &[], false, Some(t("2026-10-10T10:05:00Z")));
        assert!(f.calendars["cal"].absorbed.is_empty());
    }

    /// A check that finds a change doesn't end a run of failures: a calendar whose fetch keeps
    /// failing after a successful check still backs off.
    #[test]
    fn a_fetch_that_keeps_failing_after_a_changed_check_backs_off() {
        let now = Instant::now();
        let mut f = Freshness::default();
        f.fetched("cal", Some(t("2026-10-10T10:00:00Z")), &[], now);
        let new = [item("a", "2026-10-10T10:00:10Z")];
        assert_eq!(f.checked("cal", &new, false, None), Verdict::Changed);
        f.failed("cal", now, false);
        assert_eq!(f.checked("cal", &new, false, None), Verdict::Changed);
        f.failed("cal", now, false);
        // Two in a row: a minute, not 30 s.
        assert_eq!(f.plan("cal", now + Duration::from_secs(31)), Plan::Skip);
        assert_eq!(
            f.plan("cal", now + Duration::from_secs(61)),
            Plan::Check(t("2026-10-10T09:59:00Z"))
        );
    }

    /// Not reaching Google at all (offline) isn't Google refusing: the next check tries again, and
    /// it never grows the wait.
    #[test]
    fn being_offline_never_grows_the_wait() {
        let now = Instant::now();
        let mut f = Freshness::default();
        f.fetched("cal", Some(t("2026-10-10T10:00:00Z")), &[], now);
        for _ in 0..5 {
            f.unreachable("cal", now);
        }
        assert_eq!(f.plan("cal", now), Plan::Skip);
        assert_eq!(
            f.plan("cal", now + Duration::from_secs(11)),
            Plan::Check(t("2026-10-10T09:59:00Z"))
        );
    }

    /// Failures back off, doubling to the full sync's 15 minutes; a sign-in problem stops checks
    /// until a full sync gets through; a success clears both.
    #[test]
    fn failures_back_off_and_a_sign_in_problem_waits_for_a_full_sync() {
        assert_eq!(backoff(1), Duration::from_secs(30));
        assert_eq!(backoff(2), Duration::from_secs(60));
        assert_eq!(backoff(3), Duration::from_secs(120));
        assert_eq!(backoff(10), Duration::from_secs(900));
        assert_eq!(backoff(u32::MAX), Duration::from_secs(900));

        let now = Instant::now();
        let mut f = Freshness::default();
        f.fetched("cal", Some(t("2026-10-10T10:00:00Z")), &[], now);
        f.failed("cal", now, false);
        assert_eq!(f.plan("cal", now), Plan::Skip);
        assert_eq!(
            f.plan("cal", now + Duration::from_secs(31)),
            Plan::Check(t("2026-10-10T09:59:00Z"))
        );
        f.failed("cal", now, true);
        assert_eq!(f.plan("cal", now + Duration::from_secs(3600)), Plan::Skip);
        f.fetched("cal", Some(t("2026-10-10T11:00:00Z")), &[], now);
        assert_eq!(f.plan("cal", now), Plan::Check(t("2026-10-10T10:59:00Z")));
    }

    /// A fetch that didn't give PM Google's clock isn't repeated on every check.
    #[test]
    fn a_fetch_without_googles_clock_is_not_repeated_at_once() {
        let now = Instant::now();
        let mut f = Freshness::default();
        f.fetched("cal", None, &[], now);
        assert_eq!(f.plan("cal", now + Duration::from_secs(60)), Plan::Skip);
        assert_eq!(f.plan("cal", now + Duration::from_secs(901)), Plan::Fetch);
    }

    /// Checks are at least ten seconds apart, however often they're asked for.
    #[test]
    fn checks_are_spaced_out() {
        let now = Instant::now();
        let mut f = Freshness::default();
        assert!(f.begin_check(now));
        assert!(!f.begin_check(now + Duration::from_secs(9)));
        assert!(f.begin_check(now + Duration::from_secs(10)));
    }

    /// A calendar unticked or disconnected is forgotten.
    #[test]
    fn only_selected_calendars_are_kept() {
        let now = Instant::now();
        let mut f = Freshness::default();
        f.fetched("a", Some(t("2026-10-10T10:00:00Z")), &[], now);
        f.fetched("b", Some(t("2026-10-10T10:00:00Z")), &[], now);
        f.keep_only(&HashSet::from(["a"]));
        assert_eq!(f.plan("b", now), Plan::Fetch);
        assert_eq!(f.plan("a", now), Plan::Check(t("2026-10-10T09:59:00Z")));
    }
}
