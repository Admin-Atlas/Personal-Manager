// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Keeping every calendar event PM has seen (#1047). Bobby: "if im asking the assistant for a event
//! from last year, it shold still be retrievable", and "first download does absolutely everything".
//!
//! The mirror is split by time, the way Google splits its own calendar app (the grid loads what you
//! look at; search reaches the whole history):
//!
//! - the **live band**, from the start of last month to 13 months ahead ([`crate::calendar::
//!   time_window`]): every full sync and every change check fetch it, as before;
//! - **history**, everything before the live band: downloaded whole on a calendar's first sync, then
//!   fetched again only when Google reports a change that reaches it ([`reaches_history`]) or a write
//!   of PM's own left it behind ([`HistoryState::stale`]); an Outlook calendar, which has no change
//!   check, is fetched again once a day ([`outlook_due`]);
//! - **later**, past the live band: loaded when the calendar view scrolls to it.
//!
//! Each fetch is a complete listing of one [`Band`], and a band speaks only for the rows whose start
//! falls inside it: [`crate::calendar::replace_band`] compares what the mirror holds there with what
//! came back, writes only what changed, and removes only what the band no longer lists. Rows outside
//! the band are left alone, so a live sync never touches history and a history fetch never touches
//! the live band. (The mirror used to delete a calendar's rows and write the window back on every
//! change, which is why it couldn't keep anything older.)
//!
//! Events that ended more than a year ago keep their description and guest list compressed
//! ([`pack`]), as they're rarely read.
//!
//! Pure apart from the zstd calls in [`pack`] and [`unpack`]; `calendar` and `commands::calendars`
//! do the I/O around it.

use chrono::{DateTime, NaiveDate, TimeDelta, Utc};
use serde::{Deserialize, Serialize};

/// How far each request reaches past its band on both sides. Google and Outlook list an event when
/// it overlaps the request, and read an all-day date in the calendar's own zone, which PM doesn't
/// know when it stores the row (it reads the date as UTC midnight). Two days covers any zone
/// (UTC−12 to UTC+14) with a day to spare, so every event whose start PM files inside the band is
/// certainly in the listing, and missing from it means gone.
fn margin() -> TimeDelta {
    TimeDelta::days(2)
}

/// One stretch of time a complete listing speaks for: the rows whose start falls in `[from, to)`.
/// `None` is open on that side.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Band {
    pub from: Option<DateTime<Utc>>,
    pub to: Option<DateTime<Utc>>,
    /// Where the band starts for an all-day row (a bare date, read as UTC midnight), when that
    /// differs from `from`: an iCal feed speaks for today's all-day events from the start of the
    /// user's date, but for timed ones only from now ([`Band::of_feed_listing`]). `None`: `from`.
    pub day_from: Option<DateTime<Utc>>,
    /// The live band also speaks for a row whose start PM can't read, so a complete listing still
    /// removes one that's gone (the old wipe removed them every sync). Every fetch sees the live band,
    /// so that's the band that keeps them honest.
    pub live: bool,
}

impl Band {
    /// The live band, `[from, to)` from [`crate::calendar::time_window`].
    pub fn live(from: DateTime<Utc>, to: DateTime<Utc>) -> Self {
        Band {
            from: Some(from),
            to: Some(to),
            day_from: None,
            live: true,
        }
    }

    /// History: everything before the live band's start.
    pub fn before(to: DateTime<Utc>) -> Self {
        Band {
            from: None,
            to: Some(to),
            day_from: None,
            live: false,
        }
    }

    /// Months loaded on demand, `[from, to)`.
    pub fn between(from: DateTime<Utc>, to: DateTime<Utc>) -> Self {
        Band {
            from: Some(from),
            to: Some(to),
            day_from: None,
            live: false,
        }
    }

    /// History and the live band at once, for an iCal feed: it is downloaded whole on every sync, so
    /// its history comes with the live band at no extra cost.
    pub fn through_live(to: DateTime<Utc>) -> Self {
        Band {
            from: None,
            to: Some(to),
            day_from: None,
            live: true,
        }
    }

    /// Every row and every time at once: what a test means by a calendar's whole set.
    #[cfg(test)]
    pub const fn everything() -> Self {
        Band {
            from: None,
            to: None,
            day_from: None,
            live: true,
        }
    }

    /// The band's start for a row starting at `start`: [`Band::day_from`] for a bare date.
    fn start_for(&self, start: &str) -> Option<DateTime<Utc>> {
        let bare_date = NaiveDate::parse_from_str(start.trim(), "%Y-%m-%d").is_ok();
        if bare_date {
            self.day_from.or(self.from)
        } else {
            self.from
        }
    }

    /// The request's `[timeMin, timeMax)` (RFC 3339, UTC): the band widened by [`margin`] on each
    /// bounded side (from the earlier of its two starts).
    pub fn request(&self) -> (Option<String>, Option<String>) {
        let at = |t: DateTime<Utc>| t.to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let from = match (self.from, self.day_from) {
            (Some(f), Some(d)) => Some(f.min(d)),
            (f, _) => f,
        };
        (
            from.map(|f| at(f - margin())),
            self.to.map(|t| at(t + margin())),
        )
    }

    /// Whether a stored row whose start is `start` is this band's to keep or remove.
    pub fn holds(&self, start: &str) -> bool {
        let Some(at) = start_instant(start) else {
            return self.live;
        };
        self.start_for(start).is_none_or(|f| at >= f) && self.to.is_none_or(|t| at < t)
    }

    /// What one download of an iCal feed, asked for with [`Band::through_live`], speaks for at `now`
    /// (`today` being the user's own date then), from the listing's `(start, end, recurring)`. A feed
    /// lists what its publisher keeps, and many keep only a window (the last few weeks and the year
    /// ahead, or only what's to come), so an event that has rolled out of the feed's window hasn't
    /// been deleted, and history keeps it.
    ///
    /// - **From now on, the feed is always the one to say.** No publisher rolls out what hasn't
    ///   happened yet, so an upcoming event missing from the feed was cancelled or moved, and goes.
    ///   For an all-day row "now" is the user's date, read as UTC midnight as the row is stored
    ///   ([`Band::day_from`]): today's all-day event, and tomorrow's west of UTC, are still the
    ///   feed's, while a timed one from this morning that a feed of what's to come has dropped is
    ///   kept.
    /// - **Before that, only as far back as the feed shows it keeps**: the earliest END of its
    ///   one-off events (an end before its start reads as the start). A row that starts after a
    ///   listed one-off ended would have been listed too. Not their starts (a "season" event under
    ///   way since March would reach back over months the feed may not keep), and not repeating
    ///   events at all (a weekly meeting since 2019 is expanded from its own first day, however short
    ///   the publisher's window; [`crate::calendar::clear_unlisted_series_rows`] keeps a listed
    ///   series whole instead). A feed with no past one-offs keeps everything before today.
    ///
    /// Any other band is returned as it is.
    pub fn of_feed_listing<'a>(
        self,
        listed: impl IntoIterator<Item = (&'a str, Option<&'a str>, bool)>,
        now: DateTime<Utc>,
        today: NaiveDate,
    ) -> Band {
        if self.from.is_some() {
            return self;
        }
        let day_floor = today
            .and_hms_opt(0, 0, 0)
            .map_or(now, |midnight| midnight.and_utc().min(now));
        let earliest_end = listed
            .into_iter()
            .filter(|(_, _, recurring)| !recurring)
            .filter_map(|(start, end, _)| {
                let start = start_instant(start);
                let end = end.and_then(start_instant);
                match (start, end) {
                    (Some(s), Some(e)) => Some(e.max(s)),
                    (s, e) => e.or(s),
                }
            })
            .min();
        let back_to = |floor: DateTime<Utc>| Some(earliest_end.map_or(floor, |e| e.min(floor)));
        Band {
            from: back_to(now),
            day_from: back_to(day_floor),
            ..self
        }
    }
}

/// A mirrored start or end as an instant: RFC 3339, or an all-day `YYYY-MM-DD` read as UTC midnight,
/// or a time with no offset read as UTC — the readings SQLite's `julianday` gives the column, so the
/// SQL that narrows a band's rows and this agree.
pub fn start_instant(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    if let Ok(at) = DateTime::parse_from_rfc3339(s) {
        return Some(at.with_timezone(&Utc));
    }
    if let Ok(at) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f") {
        return Some(at.and_utc());
    }
    NaiveDate::parse_from_str(s, "%Y-%m-%d")
        .ok()
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|t| t.and_utc())
}

/// One entry of Google's change listing ([`crate::calendar::check_for_changes`]): an event (a
/// series by its own id, a changed occurrence by its own) and when it changed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Changed {
    pub id: String,
    pub updated: String,
    /// Deleted, or cancelled from its series.
    pub cancelled: bool,
}

/// What the mirror holds of one changed event ([`crate::calendar::held_of`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Held {
    /// A row of it (its own, or one of its occurrences when it's a series) starts before the live
    /// band.
    pub before: bool,
    /// The mirror has any row of it at all.
    pub any: bool,
    /// Its own row already carries this change's `updated`: PM has it (its own save, say).
    pub current: bool,
}

/// Whether a change listing reaches history, read after the live band has been fetched again, so
/// everything it lists inside the live band is already in. History must be fetched again when:
///
/// - an event PM holds before the live band changed, and its row doesn't already show the change (a
///   series counts through any of its occurrences: renaming every occurrence reaches the old ones);
/// - an event PM holds nowhere changed and wasn't deleted: it's outside the live band, so it was
///   added to the past, or moved out of the live band into it. (One added more than 13 months ahead
///   reads the same, and costs one needless fetch.)
///
/// A deletion PM holds nowhere needs nothing: the live band's fetch already removed it.
pub fn reaches_history(changes: &[Changed], held: impl Fn(&Changed) -> Held) -> bool {
    changes.iter().any(|c| {
        let h = held(c);
        (h.before && !h.current) || (!h.any && !c.cancelled)
    })
}

/// Where a calendar's history stands, kept in the `settings` table under a `calendar_history:` key.
/// No row means it has never been downloaded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryState {
    /// When history was last known to match the calendar: by Google's clock for a Google calendar
    /// (the next change check counts from it, as this computer's clock can run hours off), by this
    /// computer's for an Outlook one (only ever compared with itself).
    #[serde(with = "rfc3339")]
    pub checked_at: DateTime<Utc>,
    /// A save of PM's own reached history without landing there (a change to every occurrence of a
    /// series that began before the live band, say): fetch it again at the next check.
    #[serde(default)]
    pub stale: bool,
    /// How many times a save has marked it stale. A fetch notes the count when it starts, and
    /// clears `stale` only if no mark came in while it ran: saves don't wait for the sync lock, so a
    /// mark can land mid-fetch, about a change that fetch may not have seen.
    #[serde(default)]
    pub marks: u64,
}

/// [`HistoryState::checked_at`] as RFC 3339 text (chrono is built here without its serde support).
mod rfc3339 {
    use chrono::{DateTime, Utc};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(at: &DateTime<Utc>, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&at.to_rfc3339_opts(chrono::SecondsFormat::Secs, true))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<DateTime<Utc>, D::Error> {
        let raw = String::deserialize(d)?;
        DateTime::parse_from_rfc3339(&raw)
            .map(|t| t.with_timezone(&Utc))
            .map_err(serde::de::Error::custom)
    }
}

/// The next step for a Google calendar's history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Next {
    /// Download it whole.
    Fetch,
    /// Ask Google what changed since this time, by its clock.
    Check(DateTime<Utc>),
}

/// What a Google calendar's history needs: a first download, a fetch PM's own save asked for, or a
/// change check from when it was last known whole, set back by the change check's own lag.
pub fn next_step(state: Option<&HistoryState>) -> Next {
    match state {
        Some(s) if !s.stale => Next::Check(s.checked_at - crate::calendar_fresh::lag()),
        _ => Next::Fetch,
    }
}

/// Whether an Outlook calendar's history is due: never downloaded, left behind by a save, or a day
/// old. Outlook has no cheap change check, so once a day is how a change made there to an old event
/// arrives.
pub fn outlook_due(state: Option<&HistoryState>, now: DateTime<Utc>) -> bool {
    match state {
        None => true,
        Some(s) => s.stale || now - s.checked_at >= TimeDelta::days(1) || s.checked_at > now,
    }
}

/// How long one slice of an Outlook calendar's history is: four years, so with its margins a request
/// stays well inside the 1,825 days Graph may take for one `calendarView` (unverified: no Outlook
/// account to probe, so the safe width is used).
fn outlook_slice() -> TimeDelta {
    TimeDelta::days(4 * 365)
}

/// An Outlook calendar's history as slices that meet with no gap, newest first: four-year bands back
/// from the live band's start, then one band open at the start for everything before (its request
/// starts at [`crate::calendar::EARLIEST`], so it too is at most a slice wide). Each is a complete
/// listing of its own stretch, so a slice that fails leaves the others' rows alone.
pub fn outlook_slices(live_from: DateTime<Utc>) -> Vec<Band> {
    let floor = start_instant(crate::calendar::EARLIEST).unwrap_or(live_from);
    let mut bands = Vec::new();
    let mut to = live_from;
    while to - outlook_slice() > floor {
        bands.push(Band::between(to - outlook_slice(), to));
        to -= outlook_slice();
    }
    bands.push(Band::before(to));
    bands
}

/// Events that ended this long ago keep their description and guest list compressed ([`pack`]).
fn pack_after() -> TimeDelta {
    TimeDelta::days(365)
}

/// Whether a row (its `start` and `end`, as stored) ended more than a year before `now`. A row with
/// no readable end is judged by its start; one with neither stays as it is.
pub fn aged(start: &str, end: Option<&str>, now: DateTime<Utc>) -> bool {
    end.and_then(start_instant)
        .or_else(|| start_instant(start))
        .is_some_and(|ended| ended < now - pack_after())
}

/// zstd's level for packed fields: values are small, so a higher level buys almost nothing.
const PACK_LEVEL: i32 = 9;

/// The first byte of a packed field that compression didn't shrink: the text follows as it is. A
/// zstd frame starts with its magic number (`28 B5 2F FD`), so the two can't be confused.
const RAW: u8 = 0;

/// The most a packed field may unpack to. A description is cut at 8,000 characters (at most 32 KB of
/// UTF-8) and a guest list of a few thousand people fits well inside; anything larger isn't
/// something PM packed.
const UNPACK_MAX_BYTES: usize = 1 << 20;

/// `text` as the bytes stored for a field of an event that ended more than a year ago: compressed,
/// or — when that wouldn't make it smaller, as for a short note — the text behind a [`RAW`] byte.
/// Measured on a typical video-call invitation, compression saves about 45%; on a guest list, about
/// 85%. Either way the field is stored as a BLOB, which is how a sync knows it's done.
pub fn pack(text: &str) -> Vec<u8> {
    if let Ok(packed) = zstd::bulk::compress(text.as_bytes(), PACK_LEVEL) {
        if packed.len() < text.len() {
            return packed;
        }
    }
    let mut raw = Vec::with_capacity(text.len() + 1);
    raw.push(RAW);
    raw.extend_from_slice(text.as_bytes());
    raw
}

/// A field [`pack`] stored, back as text. `None` when it isn't one (corrupt, or too large).
pub fn unpack(bytes: &[u8]) -> Option<String> {
    let raw = match bytes.split_first() {
        Some((&RAW, text)) => text.to_vec(),
        _ => zstd::bulk::decompress(bytes, UNPACK_MAX_BYTES).ok()?,
    };
    String::from_utf8(raw).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: &str) -> DateTime<Utc> {
        start_instant(s).unwrap()
    }

    #[test]
    fn a_band_holds_starts_inside_it_and_its_requests_reach_two_days_past() {
        let live = Band::live(at("2026-10-01T00:00:00Z"), at("2027-12-01T00:00:00Z"));
        assert!(
            live.holds("2026-10-01T00:00:00Z"),
            "the first instant is in"
        );
        assert!(
            live.holds("2026-10-01"),
            "an all-day start reads as UTC midnight"
        );
        assert!(!live.holds("2026-09-30T23:59:59Z"));
        assert!(!live.holds("2027-12-01T00:00:00Z"), "the end is out");
        assert!(live.holds("2026-10-05T09:00:00+02:00"));
        assert!(
            live.holds("not a time"),
            "the live band takes an unreadable start"
        );
        assert_eq!(
            live.request(),
            (
                Some("2026-09-29T00:00:00Z".to_string()),
                Some("2027-12-03T00:00:00Z".to_string())
            )
        );

        let history = Band::before(at("2026-10-01T00:00:00Z"));
        assert!(history.holds("1998-03-04"));
        assert!(history.holds("2026-09-30T23:59:59Z"));
        assert!(!history.holds("2026-10-01"));
        assert!(!history.holds("not a time"), "only the live band takes it");
        assert_eq!(
            history.request(),
            (None, Some("2026-10-03T00:00:00Z".to_string())),
            "history has no start: the first download is everything"
        );

        let feed = Band::through_live(at("2027-12-01T00:00:00Z"));
        assert!(feed.holds("1990-01-01") && feed.holds("2027-11-30") && feed.holds("?"));
        assert!(!feed.holds("2027-12-01"));
    }

    #[test]
    fn outlook_history_comes_in_slices_that_meet_and_stay_narrow() {
        let l0 = at("2026-10-01T00:00:00Z");
        let slices = outlook_slices(l0);
        assert_eq!(slices[0].to, Some(l0), "newest first, up to the live band");
        for pair in slices.windows(2) {
            assert_eq!(pair[1].to, pair[0].from, "no gap, no overlap");
        }
        let last = slices.last().unwrap();
        assert_eq!(last.from, None, "everything before is the last slice's");
        // Every request, margins and the open slice's 1900 start included, stays under 1,825 days.
        let earliest = start_instant(crate::calendar::EARLIEST).unwrap();
        for band in &slices {
            let (from, to) = band.request();
            let from = from.map_or(earliest, |f| start_instant(&f).unwrap());
            let to = start_instant(&to.unwrap()).unwrap();
            assert!((to - from).num_days() < 1825, "{band:?}");
        }
        assert!(slices.iter().all(|b| !b.live));
    }

    /// Review of #1047: a feed that publishes only a window must not take history with it as the
    /// window moves on; it always clears what's to come that it no longer lists; and one that lists
    /// everything still clears all it no longer lists.
    #[test]
    fn a_feed_speaks_for_what_is_to_come_and_back_to_its_earliest_finished_one_off() {
        let now = at("2026-11-05T12:00:00Z");
        let today = NaiveDate::from_ymd_opt(2026, 11, 5).unwrap();
        let floor = at("2026-11-05T00:00:00Z");
        let feed = Band::through_live(at("2027-12-01T00:00:00Z"));
        let one_off = |s, e| (s, Some(e), false);
        let repeat = |s| (s, None, true);

        // A window of one-offs from 20-09, beside a weekly meeting expanded from 2019: the window
        // starts where the earliest one-off ended, so a one-off from 2023 that rolled out is kept.
        let windowed = feed.of_feed_listing(
            [
                repeat("2019-01-07T09:00:00Z"),
                one_off("2026-09-20T09:00:00Z", "2026-09-20T10:00:00Z"),
                one_off("2027-01-05", "2027-01-06"),
                repeat("2026-11-02T09:00:00Z"),
            ],
            now,
            today,
        );
        assert_eq!(windowed.from, Some(at("2026-09-20T10:00:00Z")));
        assert!(
            !windowed.holds("2023-06-01"),
            "rolled out of the feed: kept"
        );
        assert!(
            windowed.holds("2026-09-25"),
            "inside the window: the feed's to clear"
        );

        // Only what's to come: last week's event drops out and history keeps it; the next one,
        // cancelled, goes (no ghost left at its old time), today's all-day one included.
        let ahead = feed.of_feed_listing([one_off("2026-11-20", "2026-11-21")], now, today);
        assert_eq!((ahead.from, ahead.day_from), (Some(now), Some(floor)));
        assert!(!ahead.holds("2026-11-04"));
        assert!(ahead.holds("2026-11-05"), "today's all-day event");
        assert!(ahead.holds("2026-11-10"), "cancelled before it happened");
        // This morning's meeting, over and dropped by the feed, is kept; this afternoon's is the
        // feed's.
        assert!(!ahead.holds("2026-11-05T09:00:00Z"));
        assert!(ahead.holds("2026-11-05T13:00:00Z"));

        // West of UTC, in the evening, tomorrow's all-day row (06-11, UTC midnight) is before `now`
        // already, and still the feed's.
        let la_now = at("2026-11-06T02:00:00Z");
        let la = feed.of_feed_listing([], la_now, today);
        assert!(la.holds("2026-11-06") && la.holds("2026-11-05"));
        // ...while yesterday evening's timed event there, over, is kept.
        assert!(!la.holds("2026-11-06T01:00:00Z"));

        // Repeating events alone, or nothing: everything before now is kept, so a feed whose
        // window holds no one-offs this week can't reach back over its 2019 series.
        let repeats =
            feed.of_feed_listing([repeat("2019-02-05"), repeat("2026-11-02")], now, today);
        assert_eq!((repeats.from, repeats.day_from), (Some(now), Some(floor)));
        assert!(!repeats.holds("2023-06-01"));
        assert_eq!(feed.of_feed_listing([], now, today).from, Some(now));

        // A "season" event under way since March doesn't reach back over the months since: it
        // hasn't ended. Nor does one whose end the feed wrote before its start.
        let season = feed.of_feed_listing([one_off("2026-03-01", "2026-12-20")], now, today);
        assert_eq!(season.from, Some(now));
        let backwards =
            feed.of_feed_listing([one_off("2026-10-10T09:00:00Z", "2020-01-01")], now, today);
        assert_eq!(backwards.from, Some(at("2026-10-10T09:00:00Z")));

        // A feed of everything since 1999 clears what it no longer lists, unreadable starts too.
        let whole = feed.of_feed_listing(
            [one_off("1999-01-01", "1999-01-02"), ("?", None, false)],
            now,
            today,
        );
        assert!(whole.holds("2003-04-05") && whole.holds("?") && whole.live);
        let later = Band::between(now, at("2027-01-01T00:00:00Z"));
        assert_eq!(
            later.of_feed_listing([one_off("1999-01-01", "1999-01-02")], now, today),
            later
        );
    }

    #[test]
    fn the_bands_meet_without_a_gap_or_an_overlap() {
        let l0 = at("2026-10-01T00:00:00Z");
        let l1 = at("2027-12-01T00:00:00Z");
        let bands = [
            Band::before(l0),
            Band::live(l0, l1),
            Band::between(l1, at("2028-12-01T00:00:00Z")),
        ];
        for start in [
            "2026-09-30T23:59:59Z",
            "2026-10-01T00:00:00Z",
            "2026-10-01",
            "2027-11-30T23:59:59Z",
            "2027-12-01",
            "2028-11-30",
        ] {
            let holders = bands.iter().filter(|b| b.holds(start)).count();
            assert_eq!(holders, 1, "{start} belongs to exactly one band");
        }
    }

    fn changed(id: &str, cancelled: bool) -> Changed {
        Changed {
            id: id.into(),
            updated: "2026-11-05T10:00:00.000Z".into(),
            cancelled,
        }
    }

    #[test]
    fn a_change_reaches_history_only_through_an_old_row_or_one_pm_holds_nowhere() {
        let held = |old: bool, any: bool, current: bool| {
            move |_: &Changed| Held {
                before: old,
                any,
                current,
            }
        };
        let one = [changed("e", false)];
        assert!(
            reaches_history(&one, held(true, true, false)),
            "an old event or series changed"
        );
        assert!(
            !reaches_history(&one, held(true, true, true)),
            "its row already shows the change (PM's own save)"
        );
        assert!(
            !reaches_history(&one, held(false, true, false)),
            "a live event: the live fetch has it"
        );
        assert!(
            reaches_history(&one, held(false, false, false)),
            "added to the past, or moved there out of the live band"
        );
        let gone = [changed("e", true)];
        assert!(
            !reaches_history(&gone, held(false, false, false)),
            "a deletion PM holds nowhere: nothing to remove"
        );
        assert!(
            reaches_history(&gone, held(true, true, false)),
            "an old event deleted: history must drop it"
        );
        assert!(!reaches_history(&[], held(true, false, false)));
    }

    #[test]
    fn a_calendar_never_downloaded_or_left_stale_is_fetched_whole() {
        assert_eq!(next_step(None), Next::Fetch);
        let t = at("2026-11-05T10:00:00Z");
        let fresh = HistoryState {
            checked_at: t,
            stale: false,
            marks: 0,
        };
        assert_eq!(
            next_step(Some(&fresh)),
            Next::Check(t - crate::calendar_fresh::lag())
        );
        let stale = HistoryState {
            stale: true,
            ..fresh.clone()
        };
        assert_eq!(next_step(Some(&stale)), Next::Fetch);

        assert!(outlook_due(None, t));
        assert!(!outlook_due(Some(&fresh), t + TimeDelta::hours(23)));
        assert!(outlook_due(Some(&fresh), t + TimeDelta::hours(24)));
        assert!(outlook_due(Some(&stale), t));
        assert!(
            outlook_due(Some(&fresh), t - TimeDelta::hours(1)),
            "a clock set back can't hold it off for good"
        );
    }

    #[test]
    fn the_state_reads_without_the_stale_flag() {
        let s: HistoryState =
            serde_json::from_str(r#"{"checked_at":"2026-11-05T10:00:00Z"}"#).unwrap();
        assert!(!s.stale);
        let back: HistoryState = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn an_event_ages_a_year_after_it_ends() {
        let now = at("2026-11-05T10:00:00Z");
        assert!(aged(
            "2025-11-01T09:00:00Z",
            Some("2025-11-01T10:00:00Z"),
            now
        ));
        assert!(
            !aged("2025-11-01T09:00:00Z", Some("2025-11-06T10:00:00Z"), now),
            "it ran on"
        );
        assert!(aged("2025-10-30", None, now), "no end: judged by its start");
        assert!(!aged("2026-11-01", Some("2026-11-02"), now));
        assert!(!aged("?", None, now), "unreadable: left as it is");
    }

    #[test]
    fn pack_shrinks_what_it_can_keeps_the_rest_and_unpacks_both_exactly() {
        let note = "Bring the slides.";
        let kept = pack(note);
        assert_eq!(
            kept.len(),
            note.len() + 1,
            "too short to gain: stored behind one byte"
        );
        assert_eq!(unpack(&kept).as_deref(), Some(note));
        assert_eq!(unpack(&pack("")).as_deref(), Some(""));

        let invite =
            "Join Zoom Meeting\nhttps://example.zoom.us/j/1234567890\n\nMeeting ID: 123 456 7890\n"
                .repeat(6);
        let packed = pack(&invite);
        assert!(packed.len() < invite.len() / 2, "a long invitation shrinks");
        assert_eq!(unpack(&packed).as_deref(), Some(invite.as_str()));
        assert_eq!(unpack(b"not zstd at all"), None);
    }
}
