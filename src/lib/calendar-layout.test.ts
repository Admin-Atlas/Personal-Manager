// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import {
  dayDiff,
  dayKey,
  minutesFromLocalMidnight,
  occurrenceKey,
  startOfDay,
  startOfWeek,
  timedEndMinutes,
} from "./calendar-layout";
import type { CalendarEvent } from "./types";

// Local Date constructors (new Date(y, m0, d, h, min)) read the local clock, so these are TZ-agnostic.

describe("timedEndMinutes", () => {
  it("clamps an end at exactly next-midnight to 1440 (F-62 — not a 14px sliver)", () => {
    const start = new Date(2024, 2, 5, 20, 0); // 20:00
    const end = new Date(2024, 2, 6, 0, 0); // 00:00 the next day
    expect(timedEndMinutes(start, end)).toBe(1440);
  });

  it("returns the plain minute-of-day for a same-day end", () => {
    const start = new Date(2024, 2, 5, 9, 0);
    const end = new Date(2024, 2, 5, 10, 30);
    expect(timedEndMinutes(start, end)).toBe(630); // 10:30
  });

  it("defaults to a 30-minute block when there is no end", () => {
    expect(timedEndMinutes(new Date(2024, 2, 5, 9, 0), null)).toBe(570); // 09:00 + 30
  });

  it("leaves a genuine start-of-day (00:00) end alone when it isn't after the start", () => {
    const midnight = new Date(2024, 2, 5, 0, 0);
    expect(timedEndMinutes(midnight, midnight)).toBe(0);
  });
});

describe("occurrenceKey", () => {
  const ev = (over: Partial<CalendarEvent>): CalendarEvent => ({
    id: "e",
    calendar_id: "cal-1",
    summary: "Standup",
    description: null,
    location: null,
    start: "2026-07-06T09:00:00Z",
    end: null,
    all_day: false,
    html_link: null,
    uid: "u1",
    ...over,
  });

  it("is null without a UID — an uncorrelatable event is never deduped", () => {
    expect(occurrenceKey(ev({ uid: null }))).toBeNull();
  });

  it("separates the occurrences of one series (the UID alone names the series)", () => {
    const first = ev({ start: "2026-07-06T09:00:00Z" });
    const second = ev({ start: "2026-07-13T09:00:00Z" });
    expect(occurrenceKey(first)).not.toBe(occurrenceKey(second));
  });

  it("still collapses one occurrence mirrored on two calendars", () => {
    const google = ev({ id: "g", calendar_id: "cal-1" });
    const outlook = ev({ id: "o", calendar_id: "cal-2" });
    expect(occurrenceKey(google)).toBe(occurrenceKey(outlook));
  });
});

describe("minutesFromLocalMidnight", () => {
  it("is hours*60 + minutes of the local clock", () => {
    expect(minutesFromLocalMidnight(new Date(2024, 2, 5, 13, 45))).toBe(825);
    expect(minutesFromLocalMidnight(new Date(2024, 2, 5, 0, 0))).toBe(0);
  });
});

describe("startOfWeek — the Monday of the week containing the day", () => {
  it("walks back to the Monday from anywhere in the week", () => {
    // 2026-07-27 is a Monday; 2026-08-02 is the Sunday that closes the same week.
    expect(dayKey(startOfWeek(new Date(2026, 6, 27)))).toBe("2026-07-27"); // Monday → itself
    expect(dayKey(startOfWeek(new Date(2026, 7, 1)))).toBe("2026-07-27"); // Saturday
    expect(dayKey(startOfWeek(new Date(2026, 7, 2)))).toBe("2026-07-27"); // Sunday, NOT the 3rd
  });

  it("never returns a start in the future, and always one the day falls inside", () => {
    // Sunday is the trap: a `getDay()`-indexed implementation makes it day 0 and returns the
    // Monday AFTER, so the calendar opens on a week that has not happened and today is off screen.
    for (let dayShift = 0; dayShift < 14; dayShift++) {
      const d = new Date(2026, 7, 1 + dayShift);
      const start = startOfWeek(d);
      expect(start.getTime()).toBeLessThanOrEqual(startOfDay(d).getTime());
      expect(dayDiff(start, d)).toBeGreaterThanOrEqual(0);
      expect(dayDiff(start, d)).toBeLessThan(7);
      expect(start.getDay()).toBe(1); // a Monday, every time
    }
  });
});
