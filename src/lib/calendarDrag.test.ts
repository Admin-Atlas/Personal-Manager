// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The maths of moving and creating events on the calendar grid (#884, C15): snapping, where the
// pointer is, every plan a drag can make, and the pending look's overlay. Fixtures are local wall
// times (`new Date(y, m0, d, h, min)` reads the local clock), so these hold in any device zone; the
// skipped-hour checks name their zone.

import { afterEach, describe, expect, it, vi } from "vitest";

import { DRAFT_EVENT_ID, MILESTONE_CALENDAR_ID, timedSegments } from "./calendar-layout";
import {
  addWall,
  applyPending,
  autoScrollDelta,
  dragCapability,
  dropChoicesNeeded,
  exceedsThreshold,
  gapAnnouncement,
  gapIn,
  gapText,
  ghostEvent,
  grabAllowed,
  grabOf,
  handleSizes,
  inCancelZone,
  isNoop,
  NO_END,
  NO_ZONE,
  pixelToMinute,
  planBandMove,
  planBandResize,
  planBandToTimed,
  planClick,
  planClickCreate,
  planCreate,
  planDayRange,
  planGesture,
  planMonthMove,
  planResize,
  planTimedMove,
  planTimedToBand,
  prunePending,
  resolveMonthTarget,
  resolveTimeGridTarget,
  samePlan,
  SAVING,
  seenTimesOf,
  slotRect,
  snapMinutes,
  timingOf,
  toMoveTo,
  wallDiff,
  xToDayIndex,
  type AllDayTiming,
  type CapabilityContext,
  type DragCapability,
  type MonthGeom,
  type PendingMove,
  type Plan,
  type Press,
  type PressOn,
  type Target,
  type TimeGridGeom,
  type TimedTiming,
  type Timing,
  type WallPoint,
} from "./calendarDrag";
import { SPECIAL_DATE_DRAG } from "./calendarEdit/editReasons";
import type { CalendarEvent, GuestNotice } from "./types";

afterEach(() => {
  vi.useRealTimers();
});

const MON = "2026-10-12";
const D = "2026-10-13"; // a Tuesday
const WED = "2026-10-14";
const THU = "2026-10-15";
const FRI = "2026-10-16";
const SAT = "2026-10-17";

const at = (day: string, h: number, m = 0): WallPoint => ({ day, min: h * 60 + m });
const timed = (start: WallPoint, end: WallPoint): TimedTiming => ({ kind: "timed", start, end });
const allDay = (first: string, last: string): AllDayTiming => ({ kind: "all_day", first, last });
const ok = (timing: Timing, noop = false): Plan => ({ ok: true, timing, noop });

/** A local wall time as the mirror holds a timed instant. */
const iso = (day: string, h: number, m = 0) => {
  const [y, mo, d] = day.split("-").map(Number);
  return `${new Date(y, mo - 1, d, h, m).toISOString().slice(0, 19)}Z`;
};

const row = (over: Partial<CalendarEvent> = {}): CalendarEvent => ({
  id: "e1",
  calendar_id: "c1",
  summary: "Standup",
  description: null,
  location: null,
  start: iso(D, 9),
  end: iso(D, 10),
  all_day: false,
  html_link: null,
  uid: "u1",
  ...over,
});

const CAN: DragCapability = {
  move: true,
  resizeStart: true,
  resizeEnd: true,
  bandEdges: true,
  reason: null,
};

describe("snapping, pixels and days", () => {
  it("snapMinutes rounds, floors and ceils to 15", () => {
    expect([7, 8, 22, 23].map((m) => snapMinutes(m))).toEqual([0, 15, 15, 30]);
    expect(snapMinutes(29, 15, "floor")).toBe(15);
    expect(snapMinutes(16, 15, "ceil")).toBe(30);
    for (const mode of ["round", "floor", "ceil"] as const) {
      expect([0, 15, 600].map((m) => snapMinutes(m, 15, mode))).toEqual([0, 15, 600]);
    }
    // A small move back is no move: 0, never -0.
    expect(Object.is(snapMinutes(-7), 0)).toBe(true);
    expect(Object.is(snapMinutes(-14, 15, "ceil"), 0)).toBe(true);
  });

  it("pixelToMinute reads the scrolled body", () => {
    expect(pixelToMinute(140, 100, 480, 40)).toBe(780);
    expect(pixelToMinute(50, 100, 0, 40)).toBe(-75); // above the body: unclamped
  });

  it("xToDayIndex reads measured columns", () => {
    const cols = [
      { left: 60, right: 160 },
      { left: 160, right: 300 },
      { left: 300, right: 350 },
    ];
    expect(xToDayIndex(cols, 10)).toBe(0); // the hour gutter
    expect(xToDayIndex(cols, 159.9)).toBe(0);
    expect(xToDayIndex(cols, 160)).toBe(1);
    expect(xToDayIndex(cols, 320)).toBe(2);
    expect(xToDayIndex(cols, 900)).toBe(2);
    expect(xToDayIndex([], 100)).toBeNull();
  });

  it("wall arithmetic crosses midnight", () => {
    expect(addWall({ day: D, min: 1380 }, 150)).toEqual({ day: WED, min: 90 });
    expect(wallDiff({ day: D, min: 1380 }, { day: WED, min: 90 })).toBe(150);
    expect(addWall({ day: "2026-10-31", min: 1380 }, 150)).toEqual({ day: "2026-11-01", min: 90 });
    expect(addWall({ day: D, min: 30 }, -60)).toEqual({ day: MON, min: 1410 });
    // Whole days by date: the night the clocks go back is still one day.
    expect(addWall({ day: "2026-10-24", min: 600 }, 1440)).toEqual({ day: "2026-10-25", min: 600 });
    expect(wallDiff({ day: "2026-03-28", min: 0 }, { day: "2026-03-30", min: 0 })).toBe(2880);
  });

  it("exceedsThreshold: 4 px for a mouse, 6 for a pen, never for touch", () => {
    expect(exceedsThreshold(3, 0, "mouse")).toBe(false);
    expect(exceedsThreshold(4, 0, "mouse")).toBe(true);
    expect(exceedsThreshold(3, 3, "mouse")).toBe(true);
    expect(exceedsThreshold(0, 5, "pen")).toBe(false);
    expect(exceedsThreshold(0, 6, "pen")).toBe(true);
    expect(exceedsThreshold(300, 300, "touch")).toBe(false);
  });

  it("inCancelZone: left, right and above put back; below doesn't", () => {
    const w = { left: 0, top: 0, right: 400, bottom: 800 };
    expect(inCancelZone(w, -1, 100)).toBe(true);
    expect(inCancelZone(w, 400, 100)).toBe(true);
    expect(inCancelZone(w, 100, -1)).toBe(true);
    expect(inCancelZone(w, 100, 900)).toBe(false);
    expect(inCancelZone(w, 100, 100)).toBe(false);
  });

  it("autoScrollDelta", () => {
    expect(autoScrollDelta(400, 100, 700)).toBe(0);
    expect(autoScrollDelta(668, 100, 700)).toBe(0); // the edge zone's own border
    expect(autoScrollDelta(684, 100, 700)).toBe(7); // 16 px inside the bottom
    expect(autoScrollDelta(700, 100, 700)).toBe(14);
    expect(autoScrollDelta(760, 100, 700)).toBe(14); // beyond: never faster
    expect(autoScrollDelta(116, 100, 700)).toBe(-7);
    expect(autoScrollDelta(40, 100, 700)).toBe(-14);
  });
});

describe("hitting an event", () => {
  it("handleSizes: a narrow start, a wider end, and always a body", () => {
    // The end reaches 10 px (6 px was too thin to hit); the start stays 6 px, off the title.
    expect(handleSizes(60)).toEqual({ top: 6, bottom: 10 });
    // Short: no top edge, a bottom one of a third, and the rest to move it by.
    expect(handleSizes(20)).toEqual({ top: 0, bottom: 20 / 3 });
    // Just tall enough for a top edge: a quarter, a third, and the rest body.
    expect(handleSizes(24)).toEqual({ top: 6, bottom: 8 });
    // Shorter still: all body, so it can always be moved.
    expect(handleSizes(8)).toEqual({ top: 0, bottom: 0 });
  });

  it("grabOf takes the strip pressed, where its surface has it", () => {
    expect(grabOf("grid", "start")).toBe("start");
    expect(grabOf("grid", "end")).toBe("end");
    expect(grabOf("grid", undefined)).toBe("body");
    expect(grabOf("band", "left")).toBe("left");
    expect(grabOf("band", "right")).toBe("right");
    // A strip that isn't its surface's is the body, as is anything in Month.
    expect(grabOf("grid", "left")).toBe("body");
    expect(grabOf("band", "end")).toBe("body");
    expect(grabOf("month", "end")).toBe("body");
  });

  it("grabAllowed reads the capability for each part", () => {
    const none = { ...CAN, move: false, resizeStart: false, resizeEnd: false, bandEdges: false };
    for (const g of ["body", "start", "end", "left", "right"] as const) {
      expect(grabAllowed(g, CAN)).toBe(true);
      expect(grabAllowed(g, none)).toBe(false);
    }
    expect(grabAllowed("start", { ...none, resizeEnd: true })).toBe(false);
    expect(grabAllowed("end", { ...none, resizeEnd: true })).toBe(true);
  });
});

// A Week grid: three columns, the strip above the body, scrolled to 08:00.
const GRID: TimeGridGeom = {
  wrapper: { left: 0, top: 0, right: 400, bottom: 800 },
  body: { left: 60, top: 100, right: 400, bottom: 700 },
  scrollTop: 320,
  rowH: 40,
  cols: [
    { day: MON, left: 60, right: 170 },
    { day: D, left: 170, right: 290 },
    { day: WED, left: 290, right: 400 },
  ],
  band: { left: 60, top: 60, right: 400, bottom: 90, days: [MON, D, WED] },
};

// A Month stream from Monday 31-08-2026, scrolled to its eleventh week.
const MONTH: MonthGeom = {
  wrapper: { left: 0, top: 0, right: 700, bottom: 900 },
  stream: { left: 0, top: 50, right: 700, bottom: 850 },
  scrollTop: 1000,
  weekH: 100,
  gridStart: "2026-08-31",
  weeks: 60,
};

describe("targets", () => {
  it("resolveTimeGridTarget", () => {
    expect(resolveTimeGridTarget(GRID, 200, 70)).toEqual({ zone: "band", day: D });
    expect(resolveTimeGridTarget(GRID, 395, 70)).toEqual({ zone: "band", day: WED });
    // The day headers above the strip count as the strip.
    expect(resolveTimeGridTarget(GRID, 200, 30)).toEqual({ zone: "band", day: D });
    expect(resolveTimeGridTarget(GRID, 200, 140)).toEqual({ zone: "timed", day: D, minute: 540 });
    // Below the body: its bottom edge on screen (23:00 here), never a time out of sight, however
    // far below; auto-scroll moves that edge on.
    expect(resolveTimeGridTarget(GRID, 200, 720)).toEqual({ zone: "timed", day: D, minute: 1380 });
    expect(resolveTimeGridTarget(GRID, 200, 760)).toEqual({ zone: "timed", day: D, minute: 1380 });
    // Below the wrapper isn't the cancel zone, and still stays in the column's day.
    expect(resolveTimeGridTarget(GRID, 200, 2000)).toEqual({
      zone: "timed",
      day: D,
      minute: 1380,
    });
    // Scrolled to the end, the bottom edge is the day's end, and never past it.
    const end = { ...GRID, scrollTop: 360 };
    expect(resolveTimeGridTarget(end, 200, 2000)).toEqual({ zone: "timed", day: D, minute: 1440 });
    expect(resolveTimeGridTarget({ ...GRID, scrollTop: 400 }, 200, 2000)).toEqual({
      zone: "timed",
      day: D,
      minute: 1440,
    });
    expect(resolveTimeGridTarget(GRID, -5, 140)).toBeNull();
    expect(resolveTimeGridTarget(GRID, 400, 140)).toBeNull();
    expect(resolveTimeGridTarget(GRID, 200, -1)).toBeNull();
  });

  it("without a strip, above the body is the day's start", () => {
    const g = { ...GRID, band: null, scrollTop: 0 };
    expect(resolveTimeGridTarget(g, 200, 50)).toEqual({ zone: "timed", day: D, minute: 0 });
  });

  it("an unmeasured grid has no target", () => {
    expect(resolveTimeGridTarget({ ...GRID, rowH: 0 }, 200, 140)).toBeNull();
    expect(resolveTimeGridTarget({ ...GRID, cols: [] }, 200, 140)).toBeNull();
    const bare = { ...GRID, band: { left: 60, top: 60, right: 400, bottom: 90, days: [] } };
    expect(resolveTimeGridTarget(bare, 200, 70)).toBeNull();
    // A strip with no width yet reads as its first day.
    const flat = { ...GRID, band: { left: 60, top: 60, right: 60, bottom: 90, days: [MON, D] } };
    expect(resolveTimeGridTarget(flat, 200, 70)).toEqual({ zone: "band", day: MON });
  });

  it("resolveMonthTarget", () => {
    // Row 11 (10 weeks scrolled, 1.1 more), column 3: 31-08 + 80 days.
    expect(resolveMonthTarget(MONTH, 350, 160)).toEqual({ zone: "month", day: "2026-11-19" });
    expect(resolveMonthTarget(MONTH, 0, 160)).toEqual({ zone: "month", day: "2026-11-16" });
    expect(resolveMonthTarget(MONTH, 699, 160)).toEqual({ zone: "month", day: "2026-11-22" });
    // Above the stream (the weekday header): its top row on screen.
    expect(resolveMonthTarget(MONTH, 350, 20)).toEqual({ zone: "month", day: "2026-11-12" });
    // Below it: its bottom row on screen, never a week out of sight.
    expect(resolveMonthTarget(MONTH, 350, 880)).toEqual({ zone: "month", day: "2026-12-31" });
    // Scrolled past the grid: its last week.
    expect(resolveMonthTarget({ ...MONTH, weeks: 5 }, 350, 160)).toEqual({
      zone: "month",
      day: "2026-10-01",
    });
    expect(resolveMonthTarget(MONTH, -1, 160)).toBeNull();
    expect(resolveMonthTarget(MONTH, 350, -1)).toBeNull();
    expect(resolveMonthTarget({ ...MONTH, weekH: 0 }, 350, 160)).toBeNull();
    expect(resolveMonthTarget({ ...MONTH, weeks: 0 }, 350, 160)).toBeNull();
    // A stream with no width yet reads as the week's first day.
    const flat = { ...MONTH, stream: { ...MONTH.stream, right: 0 } };
    expect(resolveMonthTarget(flat, 0, 160)).toEqual({ zone: "month", day: "2026-11-16" });
  });
});

describe("timingOf", () => {
  it("reads the mirror's row in the grid's terms", () => {
    expect(timingOf(row())).toEqual(timed(at(D, 9), at(D, 10)));
    // An end at the next midnight is that day at 0.
    expect(timingOf(row({ start: iso(D, 22), end: iso(WED, 0) }))).toEqual(
      timed(at(D, 22), at(WED, 0)),
    );
    // All day: the exclusive end becomes the last day covered.
    expect(timingOf(row({ start: MON, end: THU, all_day: true }))).toEqual(allDay(MON, WED));
    expect(timingOf(row({ start: MON, end: MON, all_day: true }))).toEqual(allDay(MON, MON));
  });

  it("is null for a row that can't be moved", () => {
    expect(timingOf(row({ end: null }))).toBeNull();
    expect(timingOf(row({ start: iso(D, 10), end: iso(D, 9) }))).toBeNull();
    expect(timingOf(row({ start: "soon" }))).toBeNull();
  });
});

describe("plans", () => {
  const nine = timed(at(D, 9), at(D, 10));

  it("planTimedMove keeps the grab point and the length", () => {
    expect(planTimedMove(nine, at(D, 9, 20), at(D, 13, 7), null)).toEqual(
      ok(timed(at(D, 12, 45), at(D, 13, 45))),
    );
    // Another column: the same times there.
    expect(planTimedMove(nine, at(D, 9, 20), at(THU, 13, 7), null)).toEqual(
      ok(timed(at(THU, 12, 45), at(THU, 13, 45))),
    );
  });

  it("planTimedMove moves an off-grid event in steps", () => {
    const off = timed(at(D, 9, 10), at(D, 9, 55));
    expect(planTimedMove(off, at(D, 9, 30), at(D, 10), null)).toEqual(
      ok(timed(at(D, 9, 40), at(D, 10, 25))),
    );
  });

  it("a wobble is no move", () => {
    expect(planTimedMove(nine, at(D, 9, 30), at(D, 9, 37), null)).toEqual(ok(nine, true));
    expect(planTimedMove(nine, at(D, 9, 30), at(D, 9, 38), null)).toEqual(
      ok(timed(at(D, 9, 15), at(D, 10, 15))),
    );
  });

  it("planTimedMove crosses midnight keeping its length", () => {
    const late = timed(at(D, 21), at(D, 23));
    expect(planTimedMove(late, at(D, 21, 30), at(D, 23, 40), null)).toEqual(
      ok(timed(at(D, 23, 15), at(WED, 1, 15))),
    );
  });

  it("a grab on a later piece measures from the true start", () => {
    const night = timed(at(D, 22), at(WED, 7));
    expect(planTimedMove(night, at(WED, 2), at(THU, 3), null)).toEqual(
      ok(timed(at(WED, 23), at(THU, 8))),
    );
  });

  it("planTimedToBand", () => {
    expect(planTimedToBand(nine, at(D, 9, 30), FRI)).toEqual(ok(allDay(FRI, FRI)));
    // Three days, pressed on the second, dropped on Friday: Thursday to Saturday.
    const three = timed(at(MON, 10), at(WED, 15));
    expect(planTimedToBand(three, at(D, 12), FRI)).toEqual(ok(allDay(THU, SAT)));
    // Ending at midnight, it fills only its own day.
    expect(planTimedToBand(timed(at(D, 22), at(WED, 0)), at(D, 23), FRI)).toEqual(
      ok(allDay(FRI, FRI)),
    );
  });

  it("planBandMove uses the true start", () => {
    expect(planBandMove(allDay(MON, WED), D, FRI)).toEqual(ok(allDay(THU, SAT)));
    // Begun on Saturday 10-10, cut at the window's Monday edge and pressed there: it keeps its
    // real first day, two days on.
    expect(planBandMove(allDay("2026-10-10", D), MON, WED)).toEqual(ok(allDay(MON, THU)));
    expect(planBandMove(allDay(MON, WED), D, D)).toEqual(ok(allDay(MON, WED), true));
  });

  it("planBandToTimed takes the given length", () => {
    const day = allDay(D, D);
    expect(planBandToTimed(day, at(D, 14, 10), 45, null)).toEqual(
      ok(timed(at(D, 14), at(D, 14, 45))),
    );
    expect(planBandToTimed(day, at(D, 14, 10), 60, null)).toEqual(ok(timed(at(D, 14), at(D, 15))));
    // At the very bottom of the day it starts in the day's last slot.
    expect(planBandToTimed(day, { day: D, min: 1440 }, 60, null)).toEqual(
      ok(timed(at(D, 23, 45), at(WED, 0, 45))),
    );
    // A length that can't be read is an hour; never under a step.
    expect(planBandToTimed(day, at(D, 14), Number.NaN, null)).toEqual(
      ok(timed(at(D, 14), at(D, 15))),
    );
    expect(planBandToTimed(day, at(D, 14), 5, null)).toEqual(ok(timed(at(D, 14), at(D, 14, 15))));
  });

  it("planResize stays in its column", () => {
    // Taken right on the edge, so the edge goes where the pointer does.
    const end = (h: number, m = 0) => planResize(nine, "end", D, 600, h * 60 + m, null);
    expect(end(11, 52)).toEqual(ok(timed(at(D, 9), at(D, 11, 45))));
    expect(end(11, 53)).toEqual(ok(timed(at(D, 9), at(D, 12))));
    expect(end(8)).toEqual(ok(timed(at(D, 9), at(D, 9, 15)))); // above the start
    expect(end(25)).toEqual(ok(timed(at(D, 9), at(WED, 0)))); // past 24:00
    const start = (h: number, m = 0) => planResize(nine, "start", D, 540, h * 60 + m, null);
    expect(start(8, 10)).toEqual(ok(timed(at(D, 8, 15), at(D, 10))));
    expect(start(11)).toEqual(ok(timed(at(D, 9, 45), at(D, 10)))); // below the end
    // A target in another column changes nothing about the day.
    const press: Press = {
      pointerId: 1,
      pointerType: "mouse",
      x0: 0,
      y0: 0,
      on: eventOn({ grab: "end", origin: nine, pieceDay: D }),
      at: { zone: "timed", day: D, minute: 595 },
      dismissOnly: false,
    };
    // Taken 5 minutes above the end, the edge keeps that offset: 09:55 to 11:52 puts it at 11:57,
    // 12:00 on the grid.
    expect(planGesture(press, { zone: "timed", day: THU, minute: 712 }, OPTS)).toEqual(
      ok(timed(at(D, 9), at(D, 12))),
    );
  });

  it("planResize goes as far as the pointer has, from where in the edge it was taken", () => {
    // Taken 12 minutes above the end (inside a wide bottom edge): taking it changes nothing.
    expect(planResize(nine, "end", D, 588, 588, null)).toEqual({
      ok: true,
      timing: nine,
      noop: true,
    });
    // Nor does a wobble of under half a step, either way.
    expect(planResize(nine, "end", D, 588, 595, null)).toEqual({
      ok: true,
      timing: nine,
      noop: true,
    });
    expect(planResize(nine, "end", D, 588, 581, null)).toEqual({
      ok: true,
      timing: nine,
      noop: true,
    });
    // Down 15 minutes: the end is 15 minutes later, not the pointer's own 09:63.
    expect(planResize(nine, "end", D, 588, 603, null)).toEqual(ok(timed(at(D, 9), at(D, 10, 15))));
    // Up 15: 15 minutes earlier.
    expect(planResize(nine, "end", D, 588, 573, null)).toEqual(ok(timed(at(D, 9), at(D, 9, 45))));
    // The top edge the same way, taken 4 minutes below the start.
    expect(planResize(nine, "start", D, 544, 529, null)).toEqual(
      ok(timed(at(D, 8, 45), at(D, 10))),
    );
    // An end off the grid (09:50) stays put until the pointer has gone half a step, then lands on
    // the grid.
    const offGrid = timed(at(D, 9), at(D, 9, 50));
    expect(planResize(offGrid, "end", D, 585, 590, null)).toEqual({
      ok: true,
      timing: offGrid,
      noop: true,
    });
    expect(planResize(offGrid, "end", D, 585, 600, null)).toEqual(ok(timed(at(D, 9), at(D, 10))));
  });

  it("planResize on the last piece of a multi-day event", () => {
    const night = timed(at(D, 22), at(WED, 7));
    expect(planResize(night, "end", WED, 420, 300, null)).toEqual(ok(timed(at(D, 22), at(WED, 5))));
    expect(planResize(night, "end", WED, 420, 0, null)).toEqual(ok(timed(at(D, 22), at(WED, 0))));
    // Midnight only while it stays a step after the start.
    const late = timed(at(D, 23, 50), at(WED, 7));
    expect(planResize(late, "end", WED, 420, 0, null)).toEqual(
      ok(timed(at(D, 23, 50), at(WED, 0, 5))),
    );
    // The start edge on the first piece stays on its day.
    expect(planResize(night, "start", D, 1320, 1440, null)).toEqual(
      ok(timed(at(D, 23, 45), at(WED, 7))),
    );
  });

  it("planBandResize", () => {
    expect(planBandResize(allDay(MON, WED), "right", FRI)).toEqual(ok(allDay(MON, FRI)));
    expect(planBandResize(allDay(D, WED), "right", MON)).toEqual(ok(allDay(D, D)));
    expect(planBandResize(allDay(MON, WED), "left", FRI)).toEqual(ok(allDay(WED, WED)));
    expect(planBandResize(allDay(D, WED), "left", MON)).toEqual(ok(allDay(MON, WED)));
  });

  it("planCreate", () => {
    const anchor = at(D, 9, 7);
    expect(planCreate(anchor, 640, null)).toEqual(ok(timed(at(D, 9), at(D, 10, 45))));
    expect(planCreate(anchor, 440, null)).toEqual(ok(timed(at(D, 7, 15), at(D, 9, 15)))); // up
    expect(planCreate(anchor, 550, null)).toEqual(ok(timed(at(D, 9), at(D, 9, 15)))); // same slot
    expect(planCreate(anchor, 540, null)).toEqual(ok(timed(at(D, 9), at(D, 9, 15))));
    // Clamped to the anchor's day.
    expect(planCreate(at(D, 23, 10), 1500, null)).toEqual(ok(timed(at(D, 23), at(WED, 0))));
    expect(planCreate(at(D, 0, 30), -20, null)).toEqual(ok(timed(at(D, 0), at(D, 0, 45))));
    // A press at the very bottom anchors on the day's last slot.
    expect(planCreate({ day: D, min: 1440 }, 1440, null)).toEqual(
      ok(timed(at(D, 23, 45), at(WED, 0))),
    );
  });

  it("planClickCreate", () => {
    expect(planClickCreate(at(D, 9, 7), 60, null)).toEqual(ok(timed(at(D, 9), at(D, 10))));
    expect(planClickCreate(at(D, 23, 20), 60, null)).toEqual(
      ok(timed(at(D, 23, 15), at(WED, 0, 15))),
    );
    expect(planClickCreate(at(D, 9, 7), 30, null)).toEqual(ok(timed(at(D, 9), at(D, 9, 30))));
    expect(planClickCreate({ day: D, min: 1440 }, 60, null)).toEqual(
      ok(timed(at(D, 23, 45), at(WED, 0, 45))),
    );
  });

  it("planMonthMove and planDayRange", () => {
    const chip = timed(at(MON, 9, 30), at(MON, 10));
    expect(planMonthMove(chip, MON, THU, null)).toEqual(ok(timed(at(THU, 9, 30), at(THU, 10))));
    expect(planMonthMove(allDay(MON, D), D, "2026-10-20", null)).toEqual(
      ok(allDay("2026-10-19", "2026-10-20")),
    );
    // A timed event across days shifts both ends.
    expect(planMonthMove(timed(at(MON, 22), at(D, 2)), D, THU, null)).toEqual(
      ok(timed(at(WED, 22), at(THU, 2))),
    );
    expect(planMonthMove(chip, MON, MON, null)).toEqual(ok(chip, true));
    expect(planDayRange(WED, MON)).toEqual(ok(allDay(MON, WED)));
    expect(planDayRange(MON, WED)).toEqual(ok(allDay(MON, WED)));
  });

  it("planClick: where a click on empty space creates", () => {
    const opts = { lengthMin: 30, zone: "Europe/London" };
    expect(planClick({ zone: "timed", day: D, minute: 547 }, opts)).toEqual(
      ok(timed(at(D, 9), at(D, 9, 30))),
    );
    // No timed event without the device's zone.
    expect(planClick({ zone: "timed", day: D, minute: 547 }, { ...opts, zone: null })).toBeNull();
    expect(planClick({ zone: "band", day: D }, { ...opts, zone: null })).toEqual(ok(allDay(D, D)));
    expect(planClick({ zone: "month", day: D }, opts)).toEqual(ok(allDay(D, D)));
  });
});

const OPTS = { lengthMin: 45, zone: null };

function eventOn(over: Partial<Extract<PressOn, { kind: "event" }>> = {}): PressOn {
  return {
    kind: "event",
    row: row(),
    grab: "body",
    surface: "grid",
    origin: timed(at(D, 9), at(D, 10)),
    pieceDay: D,
    cap: CAN,
    ...over,
  };
}

const pressOf = (on: PressOn, where: Target): Press => ({
  pointerId: 1,
  pointerType: "mouse",
  x0: 0,
  y0: 0,
  on,
  at: where,
  dismissOnly: false,
});

describe("planGesture dispatches by press and target", () => {
  const nine = timed(at(D, 9), at(D, 10));
  const tue = allDay(D, D);
  const inHours: Target = { zone: "timed", day: THU, minute: 14 * 60 + 10 };
  const inStrip: Target = { zone: "band", day: FRI };
  const timedPress = pressOf(eventOn(), { zone: "timed", day: D, minute: 9 * 60 + 20 });
  const bandPress = pressOf(eventOn({ surface: "band", origin: tue }), { zone: "band", day: D });

  it("a timed event's body: the hours, or the strip", () => {
    expect(planGesture(timedPress, inHours, OPTS)).toEqual(
      planTimedMove(nine, at(D, 9, 20), at(THU, 14, 10), null),
    );
    expect(planGesture(timedPress, inStrip, OPTS)).toEqual(ok(allDay(FRI, FRI)));
  });

  it("an all-day event's body: the strip, or the hours for its account's length", () => {
    expect(planGesture(bandPress, inStrip, OPTS)).toEqual(ok(allDay(FRI, FRI)));
    const zoned = { ...OPTS, zone: "Europe/London" };
    expect(planGesture(bandPress, inHours, zoned)).toEqual(ok(timed(at(THU, 14), at(THU, 14, 45))));
    // Not into the hours without the device's zone: a timed event needs one.
    expect(planGesture(bandPress, inHours, OPTS)).toEqual({ ok: false, why: "outside" });
  });

  it("a timed edge plans in the hours and keeps its last plan over the strip", () => {
    const p = pressOf(eventOn({ grab: "start" }), { zone: "timed", day: D, minute: 541 });
    expect(planGesture(p, { zone: "timed", day: D, minute: 490 }, OPTS)).toEqual(
      ok(timed(at(D, 8, 15), at(D, 10))),
    );
    expect(planGesture(p, inStrip, OPTS)).toBeNull();
  });

  it("a strip end plans in the strip and keeps its last plan over the hours", () => {
    const p = pressOf(eventOn({ grab: "right", surface: "band", origin: tue }), {
      zone: "band",
      day: D,
    });
    expect(planGesture(p, inStrip, OPTS)).toEqual(ok(allDay(D, FRI)));
    expect(planGesture(p, inHours, OPTS)).toBeNull();
  });

  it("anything in Month moves by days", () => {
    const p = pressOf(eventOn({ surface: "month" }), { zone: "month", day: D });
    expect(planGesture(p, { zone: "month", day: FRI }, OPTS)).toEqual(
      ok(timed(at(FRI, 9), at(FRI, 10))),
    );
    // Not across surfaces.
    expect(planGesture(timedPress, { zone: "month", day: FRI }, OPTS)).toBeNull();
  });

  it("empty space creates: a range in the hours, days in the strip and Month", () => {
    const grid = pressOf(
      { kind: "empty", surface: "grid", create: ok(timed(at(D, 9), at(D, 10))) },
      { zone: "timed", day: D, minute: 547 },
    );
    // The anchor's column only, whichever the pointer is over.
    expect(planGesture(grid, { zone: "timed", day: THU, minute: 640 }, OPTS)).toEqual(
      ok(timed(at(D, 9), at(D, 10, 45))),
    );
    expect(planGesture(grid, inStrip, OPTS)).toBeNull();
    const strip = pressOf(
      { kind: "empty", surface: "band", create: ok(tue) },
      { zone: "band", day: D },
    );
    expect(planGesture(strip, inStrip, OPTS)).toEqual(ok(allDay(D, FRI)));
    expect(planGesture(strip, inHours, OPTS)).toBeNull();
    const month = pressOf(
      { kind: "empty", surface: "month", create: ok(tue) },
      { zone: "month", day: FRI },
    );
    expect(planGesture(month, { zone: "month", day: D }, OPTS)).toEqual(ok(allDay(D, FRI)));
  });

  it("nowhere to drop is outside, whatever was pressed", () => {
    expect(planGesture(timedPress, null, OPTS)).toEqual({ ok: false, why: "outside" });
    expect(planGesture(bandPress, null, OPTS)).toEqual({ ok: false, why: "outside" });
  });

  it("a press that may do nothing plans nothing", () => {
    const stuck = pressOf(eventOn({ cap: { ...CAN, move: false, reason: "No." } }), timedPress.at);
    expect(planGesture(stuck, inHours, OPTS)).toBeNull();
    const noEnd = pressOf(eventOn({ origin: null }), timedPress.at);
    expect(planGesture(noEnd, inHours, OPTS)).toBeNull();
    const noCreate = pressOf({ kind: "empty", surface: "grid", create: null }, timedPress.at);
    expect(planGesture(noCreate, inHours, OPTS)).toBeNull();
  });
});

describe("the clocks", () => {
  const LONDON = "Europe/London";

  it("gapIn finds a skipped changed half only", () => {
    // 28-03-2027: the clocks go forward at 01:00, so 01:30 never happens in London.
    const spring = "2027-03-28";
    const was = timed(at("2027-03-27", 9), at("2027-03-27", 10));
    expect(gapIn(was, timed(at(spring, 1, 30), at(spring, 2, 30)), LONDON)).toEqual(
      at(spring, 1, 30),
    );
    expect(gapIn(was, timed(at(spring, 0, 30), at(spring, 1, 15)), LONDON)).toEqual(
      at(spring, 1, 15),
    );
    // A half left where it was isn't checked.
    const held = timed(at(spring, 1, 30), at(spring, 3));
    expect(gapIn(held, timed(at(spring, 1, 30), at(spring, 4)), LONDON)).toBeNull();
    // 31-10-2027: 01:30 happens twice; allowed.
    const autumn = "2027-10-31";
    expect(gapIn(was, timed(at(autumn, 1, 30), at(autumn, 2, 30)), LONDON)).toBeNull();
    expect(gapIn(null, allDay(spring, spring), LONDON)).toBeNull();
  });

  it("a plan into the skipped hour is refused, and says where it would have gone", () => {
    const spring = "2027-03-28";
    const was = timed(at("2027-03-27", 1, 30), at("2027-03-27", 2, 30));
    const to = timed(at(spring, 1, 30), at(spring, 2, 30));
    expect(planTimedMove(was, at("2027-03-27", 2), at(spring, 2), LONDON)).toEqual({
      ok: false,
      why: "gap",
      at: at(spring, 1, 30),
      timing: to,
    });
    // No zone, no check.
    expect(planTimedMove(was, at("2027-03-27", 2), at(spring, 2), null)).toEqual(ok(to));
  });

  it("says when it doesn't happen", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date(2027, 0, 5, 12));
    expect(gapText(at("2027-03-28", 1, 30))).toBe(
      "01:30 doesn't happen on 28-03: the clocks go forward.",
    );
    expect(gapAnnouncement(at("2027-03-28", 1, 30))).toBe("01:30 doesn't happen on 28-03.");
    vi.setSystemTime(new Date(2026, 9, 23, 12));
    expect(gapAnnouncement(at("2027-03-28", 1, 30))).toBe("01:30 doesn't happen on 28-03-2027.");
  });
});

describe("plans compare by what they show", () => {
  it("samePlan", () => {
    const a = ok(timed(at(D, 9), at(D, 10)));
    expect(samePlan(a, ok(timed(at(D, 9), at(D, 10))))).toBe(true);
    expect(samePlan(a, ok(timed(at(D, 9), at(D, 10)), true))).toBe(false);
    expect(samePlan(a, ok(timed(at(D, 9), at(D, 10, 15))))).toBe(false);
    expect(samePlan(a, ok(allDay(D, D)))).toBe(false);
    expect(samePlan(null, null)).toBe(true);
    expect(samePlan(a, null)).toBe(false);
    const out: Plan = { ok: false, why: "outside" };
    expect(samePlan(out, { ok: false, why: "outside" })).toBe(true);
    expect(samePlan(out, a)).toBe(false);
    const gap = (m: number): Plan => ({
      ok: false,
      why: "gap",
      at: at(D, 1, m),
      timing: timed(at(D, 1, m), at(D, 2)),
    });
    expect(samePlan(gap(30), gap(30))).toBe(true);
    expect(samePlan(gap(30), gap(45))).toBe(false);
    expect(samePlan(gap(30), out)).toBe(false);
  });

  it("isNoop", () => {
    expect(isNoop(allDay(D, D), allDay(D, D))).toBe(true);
    expect(isNoop(allDay(D, D), allDay(D, WED))).toBe(false);
    expect(isNoop(allDay(D, D), timed(at(D, 0), at(WED, 0)))).toBe(false);
  });
});

describe("helpers", () => {
  it("ghostEvent is in mirror shapes", () => {
    const origin = row();
    const ghost = ghostEvent(origin, timed(at(D, 23), at(WED, 1, 30)));
    expect(ghost).toEqual({ ...origin, start: iso(D, 23), end: iso(WED, 1, 30), all_day: false });
    expect(ghost.start).toMatch(/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:00Z$/);
    // Its pieces are where the grid would draw them.
    const days = [D, WED].map((k) => new Date(`${k}T00:00:00`));
    expect(timedSegments(ghost, days).map((s) => [s.dayIndex, s.startMin, s.endMin])).toEqual([
      [0, 1380, 1440],
      [1, 0, 90],
    ]);
    // All day: the exclusive end.
    expect(ghostEvent(origin, allDay(D, WED))).toEqual({
      ...origin,
      start: D,
      end: THU,
      all_day: true,
    });
  });

  it("ghostEvent draws a time the clocks skip without failing", () => {
    // Skipped where the device's zone is London; any zone gives a real instant.
    const ghost = ghostEvent(row(), timed(at("2027-03-28", 1, 30), at("2027-03-28", 2, 30)));
    expect(Number.isNaN(Date.parse(ghost.start))).toBe(false);
    expect(Number.isNaN(Date.parse(ghost.end ?? ""))).toBe(false);
  });

  it("toMoveTo", () => {
    expect(toMoveTo(timed(at(D, 9, 5), at(WED, 0)))).toEqual({
      kind: "timed",
      start_date: D,
      start_time: "09:05",
      end_date: WED,
      end_time: "00:00",
    });
    expect(toMoveTo(allDay(D, WED))).toEqual({ kind: "all_day", first_day: D, last_day: WED });
  });

  it("seenTimesOf is the rendered row", () => {
    const r = row({ start: "2026-10-13T08:00:00Z", end: "2026-10-13T09:00:00Z" });
    expect(seenTimesOf(r)).toEqual({ start: r.start, end: r.end, all_day: false });
    expect(seenTimesOf(row({ start: D, end: null, all_day: true }))).toEqual({
      start: D,
      end: null,
      all_day: true,
    });
  });

  it("slotRect", () => {
    // 09:00-10:00 on Tuesday: 100 − 320 + 9 × 40 = 140, an hour (40 px) tall.
    expect(slotRect(GRID, timed(at(D, 9), at(D, 10)))).toEqual({
      left: 170,
      right: 290,
      top: 140,
      bottom: 180,
    });
    // Its first piece below the body, the next one on screen.
    expect(slotRect(GRID, timed(at(MON, 23), at(D, 10)))).toEqual({
      left: 170,
      right: 290,
      top: -220,
      bottom: 180,
    });
    // An all-day slot: its first day's slice of the strip, even begun before the window.
    const slice = 340 / 3;
    expect(slotRect(GRID, allDay(D, "2026-10-20"))).toEqual({
      left: 60 + slice,
      right: 60 + 2 * slice,
      top: 60,
      bottom: 90,
    });
    expect(slotRect(GRID, allDay("2026-10-10", D))?.left).toBe(60);
    // A Month day: its cell (row 11 is 1100 px down, 1000 scrolled).
    expect(slotRect(MONTH, allDay("2026-11-19", "2026-11-19"))).toEqual({
      left: 300,
      right: 400,
      top: 150,
      bottom: 250,
    });
    expect(slotRect(MONTH, timed(at("2026-11-19", 9), at("2026-11-19", 10)))?.top).toBe(150);
  });

  it("slotRect is null off screen", () => {
    expect(slotRect(GRID, timed(at(FRI, 9), at(FRI, 10)))).toBeNull(); // not a column
    expect(slotRect(GRID, timed(at(D, 1), at(D, 2)))).toBeNull(); // scrolled above
    expect(slotRect(GRID, timed(at(D, 23), at(D, 23, 30)))).toBeNull(); // below
    expect(slotRect({ ...GRID, band: null }, allDay(D, D))).toBeNull();
    expect(slotRect(GRID, allDay(FRI, SAT))).toBeNull();
    expect(slotRect(MONTH, allDay("2026-09-01", "2026-09-01"))).toBeNull(); // scrolled above
    expect(slotRect(MONTH, allDay("2027-01-10", "2027-01-10"))).toBeNull(); // below
    expect(slotRect({ ...MONTH, weekH: 0 }, allDay("2026-11-19", "2026-11-19"))).toBeNull();
  });
});

describe("dragCapability", () => {
  const ctx = (over: Partial<CapabilityContext> = {}): CapabilityContext => ({
    rights: { canEdit: true, canRetime: true, reason: null },
    pending: false,
    zoneKnown: true,
    ...over,
  });
  const none = (reason: string | null): DragCapability => ({
    move: false,
    resizeStart: false,
    resizeEnd: false,
    bandEdges: false,
    reason,
  });
  const THEIRS = "Someone else organises this event.";

  it("says what a press may do, and why not", () => {
    expect(dragCapability(row({ calendar_id: MILESTONE_CALENDAR_ID }), ctx())).toEqual(none(null));
    expect(dragCapability(row({ id: DRAFT_EVENT_ID }), ctx())).toEqual(none(null));
    expect(dragCapability(row(), ctx({ pending: true }))).toEqual(none(SAVING));
    const refused = { canEdit: true, canRetime: false, reason: THEIRS };
    expect(dragCapability(row(), ctx({ rights: refused }))).toEqual(none(THEIRS));
    expect(dragCapability(row({ end: null }), ctx())).toEqual(none(NO_END));
    expect(dragCapability(row(), ctx({ zoneKnown: false }))).toEqual(none(NO_ZONE));
    expect(
      dragCapability(row({ start: D, end: WED, all_day: true }), ctx({ zoneKnown: false })),
    ).toEqual({ move: true, resizeStart: false, resizeEnd: false, bandEdges: true, reason: null });
    expect(dragCapability(row(), ctx())).toEqual({
      move: true,
      resizeStart: true,
      resizeEnd: true,
      bandEdges: false,
      reason: null,
    });
  });

  it("the first reason that applies is the one said", () => {
    const overlay = row({ calendar_id: MILESTONE_CALENDAR_ID, end: null });
    expect(dragCapability(overlay, ctx({ pending: true }))).toEqual(none(null));
    const all = ctx({
      pending: true,
      rights: { canEdit: true, canRetime: false, reason: THEIRS },
      zoneKnown: false,
    });
    expect(dragCapability(row({ end: null }), all)).toEqual(none(SAVING));
    expect(dragCapability(row({ end: null }), { ...all, pending: false })).toEqual(none(THEIRS));
    expect(dragCapability(row({ end: null }), ctx({ zoneKnown: false }))).toEqual(none(NO_END));
  });

  // C20 critiques C2-M4, C1-L12 (LD14): a refused drag of a birthday says its own line by the
  // pointer, not its details' line, which reads as a drag on offer. The rule lives here, so the
  // view can only hand in the row's rights.
  it("a birthday's refused drag says it can't be dragged", () => {
    const details = "Only its notifications can change, and its date if it was added in Google.";
    const birthday = row({ event_type: "birthday" });
    expect(
      dragCapability(
        birthday,
        ctx({ rights: { canEdit: true, canRetime: false, reason: details } }),
      ),
    ).toEqual(none(SPECIAL_DATE_DRAG));
    // Editing off (no Edit) keeps the row's line.
    expect(
      dragCapability(
        birthday,
        ctx({ rights: { canEdit: false, canRetime: false, reason: "off" } }),
      ),
    ).toEqual(none("off"));
  });
});

describe("dropChoicesNeeded", () => {
  const notice: GuestNotice = {
    updated: 2,
    invited: 0,
    removed: 0,
    told_cancelled: 0,
    cancelled: false,
    people: 2,
    emails: 2,
    complete: true,
  };

  it("asks which occurrences of a repeating event, and about guests when there are some", () => {
    expect(dropChoicesNeeded(row({ series_id: "s1" }), null)).toEqual({
      scope: true,
      guests: null,
    });
    expect(dropChoicesNeeded(row({ recurring: true }), null)).toEqual({
      scope: true,
      guests: null,
    });
    expect(dropChoicesNeeded(row(), notice)).toEqual({ scope: false, guests: notice });
    expect(dropChoicesNeeded(row(), null)).toEqual({ scope: false, guests: null });
  });
});

describe("the pending look", () => {
  const moved = timed(at(D, 14), at(D, 15));
  const entry = (over: Partial<PendingMove> = {}): PendingMove => ({
    eventId: "e1",
    timing: moved,
    phase: "landed",
    mark: 3,
    until: null,
    flow: null,
    twinKey: null,
    ...over,
  });
  const live = (flows: string[] = [], now = 0) => ({ flows: new Set(flows), now });

  it("applyPending replaces only pending rows", () => {
    const a = row();
    const b = row({ id: "e2", summary: "Lunch" });
    const out = applyPending([a, b], new Map([["e1", entry()]]));
    expect(out[0]).toEqual({ ...a, start: iso(D, 14), end: iso(D, 15), all_day: false });
    expect(timingOf(out[0])).toEqual(moved);
    expect(out[1]).toBe(b);
    expect(a.start).toBe(iso(D, 9)); // the input is left as it was
    const day = applyPending([a], new Map([["e1", entry({ timing: allDay(D, WED) })]]));
    expect(day[0]).toMatchObject({ start: D, end: THU, all_day: true });
  });

  it("a pending move settles only on a read that started after it landed", () => {
    const pending = new Map([["e1", entry({ mark: 3 })]]);
    expect(prunePending(pending, [row()], 2, live())).toBe(pending);
    expect(prunePending(pending, [row()], 3, live()).size).toBe(0);
  });

  it("one asked about or being saved stays while its row is read", () => {
    const pending = new Map([
      ["e1", entry({ phase: "asking", mark: 0, flow: "f1" })],
      ["e2", entry({ eventId: "e2", phase: "saving", mark: 0 })],
    ]);
    const rows = [row(), row({ id: "e2" })];
    expect(prunePending(pending, rows, 99, live(["f1"]))).toBe(pending);
  });

  it("one asked about goes with the dialogs asking", () => {
    const pending = new Map([["e1", entry({ phase: "asking", mark: 0, flow: "f1" })]]);
    expect(prunePending(pending, [row()], 0, live(["f2"])).size).toBe(0);
    const unowned = new Map([["e1", entry({ phase: "asking", mark: 0, flow: null })]]);
    expect(prunePending(unowned, [row()], 0, live(["f1"])).size).toBe(0);
  });

  it("an id the read lacks goes in any phase", () => {
    for (const phase of ["asking", "saving", "landed"] as const) {
      const pending = new Map([["e1", entry({ phase, mark: 9, flow: "f1" })]]);
      expect(prunePending(pending, [row({ id: "e2" })], 0, live(["f1"])).size).toBe(0);
    }
  });

  it("one that landed without a refresh waits for a read that shows it, or its time", () => {
    const pending = new Map([["e1", entry({ mark: null, until: 1000 })]]);
    // The read still has the old time: kept, whatever the mark.
    expect(prunePending(pending, [row()], 99, live([], 999))).toBe(pending);
    // A read that shows the new time lets it go.
    const shown = row({ start: iso(D, 14), end: iso(D, 15) });
    expect(prunePending(pending, [shown], 0, live([], 0)).size).toBe(0);
    // So does its time running out.
    expect(prunePending(pending, [row()], 0, live([], 1000)).size).toBe(0);
  });
});
