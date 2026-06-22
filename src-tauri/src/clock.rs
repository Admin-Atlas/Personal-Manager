// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Civil-day reasoning in a user-chosen IANA zone. PM stores every instant in UTC,
//! but "today", "due soon", and the briefing's / agenda's "now" are *civil-day*
//! notions that must be computed in the user's zone — otherwise the day boundary
//! jumps with the machine offset (the V1 limitation: deadlines reasoned in the OS
//! zone while everything else reasoned in UTC, so "today" could land a day off well
//! away from UTC). These helpers turn a UTC instant + a `Tz` into the zone-local
//! date / "now" string the SQL deltas and prompts use, so the boundary is chosen
//! once in Rust and the SQL stays pure. Each public fn has an `*_at` twin taking an
//! injected `DateTime<Utc>`, so the zone math unit-tests without a real clock.

use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;

/// The user's civil date in `zone` at instant `now` (e.g. `2026-06-22`) — the single
/// "today" boundary the focus-view deltas reason against. `now` is injected so the
/// zone math unit-tests without a real clock.
pub fn today_in_at(zone: Tz, now: DateTime<Utc>) -> NaiveDate {
    now.with_timezone(&zone).date_naive()
}

/// The zone-local today as a SQLite-friendly `YYYY-MM-DD` string. Bound as the
/// single `:today` parameter the deadline + activity deltas share, so both reason
/// about one midnight (replacing the old `date('now','localtime')` /
/// `julianday('now')` split). `julianday(:today)` is that local midnight as a Julian
/// day, so a stored date minus `:today` is a whole-day delta on one boundary.
pub fn today_sql_in(zone: Tz) -> String {
    today_sql_in_at(zone, Utc::now())
}

/// [`today_sql_in`] with an injected instant (test seam).
pub fn today_sql_in_at(zone: Tz, now: DateTime<Utc>) -> String {
    today_in_at(zone, now).format("%Y-%m-%d").to_string()
}

/// The user's wall-clock "now" as `%Y-%m-%dT%H:%M` in `zone` — what the briefing
/// snapshot and chat agenda print as the current time.
pub fn now_local_iso(zone: Tz) -> String {
    now_local_iso_at(zone, Utc::now())
}

/// [`now_local_iso`] with an injected instant (test seam).
pub fn now_local_iso_at(zone: Tz, now: DateTime<Utc>) -> String {
    now.with_timezone(&zone).format("%Y-%m-%dT%H:%M").to_string()
}

/// Render a stored event start for display in `zone`. A timed instant (RFC3339 —
/// `…Z` or with an offset, as both the ICS and Google paths store) is converted to
/// the user's zone and formatted `%Y-%m-%dT%H:%M`; an all-day `YYYY-MM-DD` (or any
/// value that isn't a parseable instant) is returned unchanged. So an agenda the
/// model reads is one coherent zone — its "now" and its event times agree, and
/// "what's on at 3pm?" reasons in the user's clock, not UTC.
pub fn to_zone_display(start: &str, zone: Tz) -> String {
    match DateTime::parse_from_rfc3339(start) {
        Ok(dt) => dt.with_timezone(&zone).format("%Y-%m-%dT%H:%M").to_string(),
        Err(_) => start.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn today_uses_user_zone_not_utc() {
        // 2026-06-22 03:00 UTC is still 2026-06-21 23:00 in New York (EDT, -04).
        use chrono_tz::America::New_York;
        let now = Utc.with_ymd_and_hms(2026, 6, 22, 3, 0, 0).unwrap();
        assert_eq!(today_in_at(New_York, now), NaiveDate::from_ymd_opt(2026, 6, 21).unwrap());
        assert_eq!(today_sql_in_at(New_York, now), "2026-06-21");
        // The very same instant is already the 22nd in UTC — proving the split.
        assert_eq!(today_sql_in_at(Tz::UTC, now), "2026-06-22");
    }

    #[test]
    fn today_handles_a_dst_day_without_panicking() {
        // US spring-forward 2026-03-08 (local 02:00→03:00). 07:30 UTC = 02:30 EST
        // (pre-jump, -05) → the civil date is still Mar 8; the math must not panic.
        use chrono_tz::America::New_York;
        let now = Utc.with_ymd_and_hms(2026, 3, 8, 7, 30, 0).unwrap();
        assert_eq!(today_in_at(New_York, now), NaiveDate::from_ymd_opt(2026, 3, 8).unwrap());
    }

    #[test]
    fn now_local_iso_formats_wall_clock_in_zone() {
        use chrono_tz::Asia::Tokyo;
        let now = Utc.with_ymd_and_hms(2026, 6, 22, 0, 0, 0).unwrap(); // 09:00 JST
        assert_eq!(now_local_iso_at(Tokyo, now), "2026-06-22T09:00");
    }

    #[test]
    fn to_zone_display_converts_timed_and_passes_through_all_day() {
        use chrono_tz::America::New_York;
        // 15:00Z → 11:00 in New York (EDT, -04).
        assert_eq!(to_zone_display("2026-06-20T15:00:00Z", New_York), "2026-06-20T11:00");
        // An offset instant is honoured too: 15:00+01:00 = 14:00Z = 10:00 EDT.
        assert_eq!(to_zone_display("2026-06-20T15:00:00+01:00", New_York), "2026-06-20T10:00");
        // An all-day date has no instant — returned unchanged in any zone.
        assert_eq!(to_zone_display("2026-06-20", New_York), "2026-06-20");
        assert_eq!(to_zone_display("2026-06-20", Tz::UTC), "2026-06-20");
    }
}
