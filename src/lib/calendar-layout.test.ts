// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import {
  dayDiff,
  dayKey,
  inTimeGridBand,
  isEventPast,
  isMultiDay,
  minutesFromLocalMidnight,
  occurrenceKey,
  startOfDay,
  startOfWeek,
  timedEndMinutes,
  timedSegments,
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

// Timed values without a zone ("…T22:00:00") are read as local time, like the Date constructors above.
const timedEv = (start: string, end: string | null): CalendarEvent => ({
  id: "e",
  calendar_id: "c",
  summary: "Night shift",
  description: null,
  location: null,
  start,
  end,
  all_day: false,
  html_link: null,
  uid: null,
});
const week = (y: number, m0: number, d: number, n = 7) =>
  Array.from({ length: n }, (_, i) => new Date(y, m0, d + i));

describe("timedSegments — a timed event fills its hours on every day it touches", () => {
  it("splits an evening-to-morning event at midnight", () => {
    const ev = timedEv("2026-10-12T22:00:00", "2026-10-13T02:00:00");
    expect(timedSegments(ev, week(2026, 9, 12, 2))).toEqual([
      { dayIndex: 0, startMin: 1320, endMin: 1440, continuesBefore: false, continuesAfter: true },
      { dayIndex: 1, startMin: 0, endMin: 120, continuesBefore: true, continuesAfter: false },
    ]);
  });

  it("fills the whole of every day in between", () => {
    const ev = timedEv("2026-10-12T09:00:00", "2026-10-14T17:00:00");
    const segs = timedSegments(ev, week(2026, 9, 12, 3));
    expect(segs.map((s) => [s.startMin, s.endMin])).toEqual([
      [540, 1440],
      [0, 1440],
      [0, 1020],
    ]);
    expect(segs[1]).toMatchObject({ continuesBefore: true, continuesAfter: true });
  });

  it("keeps an event that ends at midnight to its own day (F-62)", () => {
    const ev = timedEv("2026-10-12T20:00:00", "2026-10-13T00:00:00");
    expect(timedSegments(ev, week(2026, 9, 12, 2))).toEqual([
      { dayIndex: 0, startMin: 1200, endMin: 1440, continuesBefore: false, continuesAfter: false },
    ]);
  });

  it("only gives pieces for the days on screen, still marked as continuing", () => {
    // Runs Sunday to Tuesday; the window is Monday alone.
    const ev = timedEv("2026-10-11T20:00:00", "2026-10-13T08:00:00");
    expect(timedSegments(ev, week(2026, 9, 12, 1))).toEqual([
      { dayIndex: 0, startMin: 0, endMin: 1440, continuesBefore: true, continuesAfter: true },
    ]);
    // A months-long event costs only the window.
    const long = timedEv("2026-01-01T09:00:00", "2026-12-31T09:00:00");
    expect(timedSegments(long, week(2026, 9, 12))).toHaveLength(7);
  });

  it("cuts a missing end's 30-minute block at midnight, and keeps an end before the start", () => {
    expect(timedSegments(timedEv("2026-10-12T23:45:00", null), week(2026, 9, 12, 2))).toEqual([
      { dayIndex: 0, startMin: 1425, endMin: 1440, continuesBefore: false, continuesAfter: false },
    ]);
    const backwards = timedSegments(
      timedEv("2026-10-12T10:00:00", "2026-10-11T09:00:00"),
      week(2026, 9, 11, 3),
    );
    expect(backwards).toHaveLength(1);
    expect(backwards[0]).toMatchObject({ dayIndex: 1, startMin: 600 });
  });

  it("leaves all-day events to the strip", () => {
    const allDay = { ...timedEv("2026-10-12", "2026-10-14"), all_day: true };
    expect(timedSegments(allDay, week(2026, 9, 12))).toEqual([]);
    expect(inTimeGridBand(allDay)).toBe(true);
    expect(inTimeGridBand(timedEv("2026-10-12T22:00:00", "2026-10-14T02:00:00"))).toBe(false);
    // Month and the agendas still draw it as one bar across its days.
    expect(isMultiDay(timedEv("2026-10-12T22:00:00", "2026-10-13T02:00:00"))).toBe(true);
  });
});

describe("isEventPast", () => {
  it("greys a timed event across days once it ends, not at the end of its last day", () => {
    const ev = timedEv("2026-10-12T22:00:00", "2026-10-13T02:00:00");
    expect(isEventPast(ev, new Date(2026, 9, 13, 1, 59))).toBe(false);
    expect(isEventPast(ev, new Date(2026, 9, 13, 2, 1))).toBe(true);
  });

  it("greys an all-day event only once its last day is over", () => {
    const ev = { ...timedEv("2026-10-12", "2026-10-14"), all_day: true };
    expect(isEventPast(ev, new Date(2026, 9, 13, 23, 0))).toBe(false);
    expect(isEventPast(ev, new Date(2026, 9, 14, 0, 1))).toBe(true);
  });
});
