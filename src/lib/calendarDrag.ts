// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The pure half of moving and creating events on the calendar grid (#884, C15): pixels to times,
// where a drop lands, and what it changes. The views measure their boxes every frame and hand them
// in, so nothing here touches the DOM; the device's zone is always passed in, for the one check that
// needs it (a time the clocks skip). Its gesture is `calendarGesture.ts`; the views take both up
// with moving (C16).
//
// The wall-clock rule. The grid draws local wall time: a day, and minutes since its midnight
// (`minutesFromLocalMidnight`). Plans keep those wall minutes, so what the ghost shows is what is
// sent, and on the day the clocks change a moved event's real length can differ from its drawn
// length by the hour that day gains or loses. A drop whose changed half falls in the hour the
// device's zone skips is refused (the ghost says why, and letting go does nothing); an hour that
// happens twice is allowed, and the backend takes its first (R6).

import {
  addDays,
  dayDiff,
  dayKey,
  isDraftEvent,
  isOverlayEvent,
  minutesFromLocalMidnight,
  parseLocal,
} from "./calendar-layout";
import { DEFAULT_LENGTH_MS } from "./calendarEdit/eventDraft";
import { isRepeating } from "./calendarEdit/saveChoices";
import { formatDateOnly, pad2 } from "./format";
import type { CalendarEvent, GuestNotice, MoveTo, SeenTimes } from "./types";
import { resolveWallTime } from "./wallTime";

// --- types -----------------------------------------------------------------------------------------

/** A local wall-clock moment as the grid draws it: a local day (`dayKey`, `YYYY-MM-DD`) and minutes
 *  since its midnight (0-1439; an end at midnight is the next day at 0). */
export interface WallPoint {
  day: string;
  min: number;
}

/** When an event happens, in the grid's own terms. `last` is the last day covered (inclusive), as
 *  the editor's all-day fields and the backend's draft hold it. */
export type Timing =
  | { kind: "timed"; start: WallPoint; end: WallPoint }
  | { kind: "all_day"; first: string; last: string };
export type TimedTiming = Extract<Timing, { kind: "timed" }>;
export type AllDayTiming = Extract<Timing, { kind: "all_day" }>;

/** Where the pointer is, read from live geometry each frame. A timed target's `minute` is clamped to
 *  its column's day (0-1440): the pointer can't carry a drop into a day it isn't over, though an
 *  event can still cross midnight by where it was grabbed. */
export type Target =
  | { zone: "timed"; day: string; minute: number }
  | { zone: "band"; day: string }
  | { zone: "month"; day: string };

export type Surface = "grid" | "band" | "month";
/** Which part of an event a press took. */
export type Grab = "body" | "start" | "end" | "left" | "right";

export interface Box {
  left: number;
  top: number;
  right: number;
  bottom: number;
}

/** The time grid, measured this frame. `cols` are the day columns as laid out now (a swipe or a page
 *  mid-drag just reads differently); `band` is the all-day strip's lane area, with its days. */
export interface TimeGridGeom {
  wrapper: Box;
  /** The scrolling time body's viewport. */
  body: Box;
  scrollTop: number;
  /** Pixels per hour, from the body's `data-row-h`. */
  rowH: number;
  cols: readonly { day: string; left: number; right: number }[];
  band: (Box & { days: readonly string[] }) | null;
}

/** Month, measured this frame: fixed-height week rows from `gridStart` (a Monday). */
export interface MonthGeom {
  wrapper: Box;
  stream: Box;
  scrollTop: number;
  weekH: number;
  gridStart: string;
  weeks: number;
}

/** What a press on an event may do. */
export interface DragCapability {
  move: boolean;
  /** The top edge (on the first piece only). */
  resizeStart: boolean;
  /** The bottom edge (on the last piece only). */
  resizeEnd: boolean;
  /** The left and right ends of an all-day event in the Week and Day strip. */
  bandEdges: boolean;
  /** Shown by the pointer when nothing may be done; null shows nothing (an overlay, the draft). */
  reason: string | null;
}

export type Plan =
  | { ok: true; timing: Timing; noop: boolean }
  /** `at` is a time the clocks skip in the device's zone; `timing` is where it would have gone, so
   *  the refused ghost can still be drawn there. */
  | { ok: false; why: "gap"; at: WallPoint; timing: Timing }
  /** Nothing to drop on (the cancel zone, or no target). */
  | { ok: false; why: "outside" };

/** What the press was on, fixed at press time. */
export type PressOn =
  | {
      kind: "event";
      /** The row as drawn when the press went down. The drop is made from it, never from a row read
       *  again at the release, so a change that arrives mid-drag can't change what was seen. */
      row: CalendarEvent;
      grab: Grab;
      surface: Surface;
      /** `timingOf(row)`; null for a row that can't be moved (no end), which `cap` says. */
      origin: Timing | null;
      /** The day of the piece pressed, for a resize (it stays in that column). */
      pieceDay: string;
      cap: DragCapability;
    }
  | {
      kind: "empty";
      surface: Surface;
      /** What a click here creates (`planClick`), or null where creating isn't allowed; a drag
       *  creates only where a click may. */
      create: Plan | null;
    };

export interface Press {
  pointerId: number;
  pointerType: string;
  x0: number;
  y0: number;
  on: PressOn;
  /** Where the press went down: a wall point in the time grid, a day in the strip or Month. */
  at: Target;
  /** A popover was open when it went down: this press only closes it, whatever it was on. */
  dismissOnly: boolean;
}

/** What a frame's plan needs besides the press and the target. */
export interface PlanOpts {
  /** How long an event dropped from the all-day strip into the hours becomes (its account's
   *  default length), or a click's new event (the create calendar's). */
  lengthMin: number;
  /** The device's zone, for the skipped-hour check; null skips it. */
  zone: string | null;
}

// --- constants -------------------------------------------------------------------------------------

export const SNAP_MIN = 15;
export const MIN_DURATION_MIN = 15;
/** The length when no account's default is known: an hour, as the editor's. */
export const DEFAULT_LENGTH_MIN = DEFAULT_LENGTH_MS / 60_000;
export const AUTOSCROLL_EDGE_PX = 32;
/** Pixels per frame. */
export const AUTOSCROLL_MAX_PX = 14;
/** How far a press moves before it's a drag. Touch never drags (a tap may still create). */
export const THRESHOLD_PX = { mouse: 4, pen: 6 } as const;
/** How far into an event the edge that starts it reaches, at most (a card's top, an all-day bar's
 *  left end). No further: the title sits there, and taking an event by its title moves it. */
export const HANDLE_MAX_PX = 6;
/** How far the edge that ends it reaches (a card's bottom, a bar's right end). 6 px proved too thin
 *  to hit: a press a little way up from the bottom moved the event instead of changing its length. */
export const END_HANDLE_MAX_PX = 10;
/** An edge takes at most one part in this many of its card or bar, so a short one keeps a body to
 *  move it by: a quarter for the start, a third for the end. */
export const HANDLE_PARTS = 4;
export const END_HANDLE_PARTS = 3;
export const TOP_HANDLE_MIN_H = 24;
export const BOTTOM_HANDLE_MIN_H = 10;

const DAY_MIN = 1440;
/** The last slot a start can take on its own day. */
const LAST_SLOT = DAY_MIN - SNAP_MIN;

// --- words (shown by the pointer and in the ghost) ---------------------------------------------------

/** Minutes since midnight as 24-hour `HH:MM`. */
const clockOf = (min: number) => `${pad2(Math.floor(min / 60))}:${pad2(Math.floor(min % 60))}`;

export const SAVING = "Saving…";
export const NO_ZONE =
  "PM can't tell this computer's time zone, so it can't move timed events. Set it in your system settings.";
export const NO_END = "This event has no end time, so it can only be changed in Edit.";

/** The refused ghost's second line. */
export function gapText(at: WallPoint): string {
  return `${clockOf(at.min)} doesn't happen on ${formatDateOnly(at.day)}: the clocks go forward.`;
}

/** What a screen reader hears when a drop in a skipped hour is let go. */
export function gapAnnouncement(at: WallPoint): string {
  return `${clockOf(at.min)} doesn't happen on ${formatDateOnly(at.day)}.`;
}

// --- snapping, pixels, days --------------------------------------------------------------------------

const clamp = (n: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, n));

export function snapMinutes(
  min: number,
  step = SNAP_MIN,
  mode: "round" | "floor" | "ceil" = "round",
): number {
  const f = mode === "floor" ? Math.floor : mode === "ceil" ? Math.ceil : Math.round;
  // `|| 0`: never -0, which a strict compare tells apart from 0.
  return f(min / step) * step || 0;
}

/** Minutes since midnight at `clientY` in a time body scrolled to `scrollTop`; unclamped. */
export function pixelToMinute(
  clientY: number,
  bodyTop: number,
  scrollTop: number,
  rowH: number,
): number {
  return ((clientY - bodyTop + scrollTop) / rowH) * 60;
}

/** The column whose `[left, right)` holds `x`, read from the measured columns (each is its own
 *  width): left of the first is the first, right of the last the last, none is null. */
export function xToDayIndex(
  cols: readonly { left: number; right: number }[],
  x: number,
): number | null {
  if (cols.length === 0) return null;
  for (let i = 0; i < cols.length; i++) if (x < cols[i].right) return i;
  return cols.length - 1;
}

/** A day key as the local midnight it names. */
function dayDate(day: string): Date {
  const [y, m, d] = day.split("-").map(Number);
  return new Date(y, m - 1, d);
}

const shiftDay = (day: string, n: number) => dayKey(addDays(dayDate(day), n));
const daysBetween = (a: string, b: string) => dayDiff(dayDate(a), dayDate(b));

/** Wall minutes from `a` to `b`. */
export function wallDiff(a: WallPoint, b: WallPoint): number {
  return daysBetween(a.day, b.day) * DAY_MIN + (b.min - a.min);
}

/** `p` moved by `minutes` of wall time, across days (whole days by date, so the clocks changing
 *  doesn't shift it). */
export function addWall(p: WallPoint, minutes: number): WallPoint {
  const total = p.min + minutes;
  const days = Math.floor(total / DAY_MIN);
  return { day: shiftDay(p.day, days), min: total - days * DAY_MIN };
}

export function exceedsThreshold(dx: number, dy: number, pointerType: string): boolean {
  if (pointerType === "touch") return false;
  const px = pointerType === "pen" ? THRESHOLD_PX.pen : THRESHOLD_PX.mouse;
  return Math.hypot(dx, dy) >= px;
}

/** Letting go here puts it back: left or right of the calendar, or above it. Below isn't, so going
 *  past the bottom while it scrolls down never puts it back. */
export function inCancelZone(wrapper: Box, x: number, y: number): boolean {
  return x < wrapper.left || x >= wrapper.right || y < wrapper.top;
}

/** Pixels to scroll this frame with the pointer at `y` over a body spanning `top`-`bottom`: none in
 *  the middle, faster the deeper into the edge, the most beyond it; negative at the top. */
export function autoScrollDelta(y: number, top: number, bottom: number): number {
  const speed = (depth: number) =>
    Math.min(AUTOSCROLL_MAX_PX, Math.ceil((AUTOSCROLL_MAX_PX * depth) / AUTOSCROLL_EDGE_PX));
  const intoBottom = y - (bottom - AUTOSCROLL_EDGE_PX);
  const intoTop = top + AUTOSCROLL_EDGE_PX - y;
  if (intoBottom > 0 && intoBottom >= intoTop) return speed(intoBottom);
  if (intoTop > 0) return -speed(intoTop);
  return 0;
}

// --- hitting an event ------------------------------------------------------------------------------

/** How tall a card's edges are: the strips drawn on it, which show the resize cursor and are what a
 *  press takes as the edge (`useCalendarGesture`'s `pressOf`). A short card keeps its body, so it can
 *  always be moved. */
export function handleSizes(heightPx: number): { top: number; bottom: number } {
  return {
    top: heightPx >= TOP_HANDLE_MIN_H ? Math.min(HANDLE_MAX_PX, heightPx / HANDLE_PARTS) : 0,
    bottom:
      heightPx >= BOTTOM_HANDLE_MIN_H
        ? Math.min(END_HANDLE_MAX_PX, heightPx / END_HANDLE_PARTS)
        : 0,
  };
}

/** What a press on an event's strip `edge` (its `data-edge`) took on `surface`: that edge, where
 *  the surface has it, else the body. */
export function grabOf(surface: Surface, edge: string | undefined): Grab {
  if (surface === "grid" && (edge === "start" || edge === "end")) return edge;
  if (surface === "band" && (edge === "left" || edge === "right")) return edge;
  return "body";
}

/** Whether a press on `grab` may do anything. */
export function grabAllowed(grab: Grab, cap: DragCapability): boolean {
  switch (grab) {
    case "body":
      return cap.move;
    case "start":
      return cap.resizeStart;
    case "end":
      return cap.resizeEnd;
    case "left":
    case "right":
      return cap.bandEdges;
  }
}

// --- targets ---------------------------------------------------------------------------------------

/** Where the pointer is over Week or Day: the all-day strip (the day headers above it count as the
 *  strip), or a time in a column, always one on screen (below the hours reads as their bottom edge,
 *  which auto-scroll then moves, as Month's bottom row does). Null in the cancel zone, or with
 *  nothing measured. */
export function resolveTimeGridTarget(g: TimeGridGeom, x: number, y: number): Target | null {
  if (inCancelZone(g.wrapper, x, y)) return null;
  if (g.band && y < g.band.bottom) {
    const n = g.band.days.length;
    if (n === 0) return null;
    const width = g.band.right - g.band.left;
    // The strip's days are equal slices of its width.
    const i = width > 0 ? clamp(Math.floor((x - g.band.left) / (width / n)), 0, n - 1) : 0;
    return { zone: "band", day: g.band.days[i] };
  }
  const i = xToDayIndex(g.cols, x);
  if (i === null || !(g.rowH > 0)) return null;
  const yIn = clamp(y, g.body.top, Math.max(g.body.top, g.body.bottom));
  const minute = clamp(pixelToMinute(yIn, g.body.top, g.scrollTop, g.rowH), 0, DAY_MIN);
  return { zone: "timed", day: g.cols[i].day, minute };
}

/** Where the pointer is over Month: a day cell, always one on screen (above the stream reads as its
 *  top row, below as its bottom one, which auto-scroll then moves). */
export function resolveMonthTarget(g: MonthGeom, x: number, y: number): Target | null {
  if (inCancelZone(g.wrapper, x, y)) return null;
  if (g.weeks <= 0 || !(g.weekH > 0)) return null;
  const width = g.stream.right - g.stream.left;
  const yIn = clamp(y, g.stream.top, Math.max(g.stream.top, g.stream.bottom - 1));
  const row = clamp(Math.floor((yIn - g.stream.top + g.scrollTop) / g.weekH), 0, g.weeks - 1);
  const col = width > 0 ? clamp(Math.floor((x - g.stream.left) / (width / 7)), 0, 6) : 0;
  return { zone: "month", day: shiftDay(g.gridStart, row * 7 + col) };
}

// --- plans -----------------------------------------------------------------------------------------

const samePoint = (a: WallPoint, b: WallPoint) => a.day === b.day && a.min === b.min;

/** Whether two timings are the same time. */
export function isNoop(a: Timing, b: Timing): boolean {
  if (a.kind === "timed" && b.kind === "timed") {
    return samePoint(a.start, b.start) && samePoint(a.end, b.end);
  }
  if (a.kind === "all_day" && b.kind === "all_day") return a.first === b.first && a.last === b.last;
  return false;
}

/** Whether two plans would look the same (a frame that changes nothing changes no state). */
export function samePlan(a: Plan | null, b: Plan | null): boolean {
  if (a === null || b === null) return a === b;
  if (a.ok && b.ok) return a.noop === b.noop && isNoop(a.timing, b.timing);
  if (!a.ok && !b.ok) {
    if (a.why === "gap" && b.why === "gap") {
      return samePoint(a.at, b.at) && isNoop(a.timing, b.timing);
    }
    return a.why === b.why;
  }
  return false;
}

/** `timing` as a plan: refused when a half it changed is in the hour the clocks skip in `zone`. */
function finish(origin: Timing | null, timing: Timing, zone: string | null): Plan {
  const at = zone === null ? null : gapIn(origin, timing, zone);
  if (at) return { ok: false, why: "gap", at, timing };
  return { ok: true, timing, noop: origin !== null && isNoop(origin, timing) };
}

/** The last day a timed event fills: one ending at midnight stops the day before, as the grid draws
 *  it (`eventDaySpan`'s rule). */
function lastDayOf(t: TimedTiming): string {
  const last =
    t.end.min === 0 && wallDiff(t.start, t.end) > 0 ? shiftDay(t.end.day, -1) : t.end.day;
  return daysBetween(t.start.day, last) < 0 ? t.start.day : last;
}

/** A move in the hours: the event goes a whole number of 15-minute steps from where it is (the
 *  pointer's travel since the press, snapped), keeping its wall length. No clamp: it may cross
 *  midnight, and `press` is anywhere on any of its pieces. */
export function planTimedMove(
  origin: TimedTiming,
  press: WallPoint,
  now: WallPoint,
  zone: string | null,
): Plan {
  const delta = snapMinutes(wallDiff(press, now));
  const timing: Timing = {
    kind: "timed",
    start: addWall(origin.start, delta),
    end: addWall(origin.end, delta),
  };
  return finish(origin, timing, zone);
}

/** A timed event dropped on the all-day strip: all day over as many days as it touches, the pressed
 *  day landing on `day`. */
export function planTimedToBand(origin: TimedTiming, press: WallPoint, day: string): Plan {
  const span = daysBetween(origin.start.day, lastDayOf(origin));
  const first = shiftDay(day, -daysBetween(origin.start.day, press.day));
  return finish(origin, { kind: "all_day", first, last: shiftDay(first, span) }, null);
}

/** An all-day event moved along the strip by the days between the day pressed (the real day under
 *  the press, not where a bar cut at the window's edge starts) and `day`. */
export function planBandMove(origin: AllDayTiming, pressDay: string, day: string): Plan {
  const n = daysBetween(pressDay, day);
  return finish(
    origin,
    { kind: "all_day", first: shiftDay(origin.first, n), last: shiftDay(origin.last, n) },
    null,
  );
}

/** A usable length: at least a step, an hour when unknown. */
const lengthOf = (lengthMin: number) =>
  Number.isFinite(lengthMin) ? Math.max(MIN_DURATION_MIN, lengthMin) : DEFAULT_LENGTH_MIN;

/** An all-day event dropped in the hours: from the slot under the pointer, `lengthMin` long. */
export function planBandToTimed(
  origin: AllDayTiming,
  now: WallPoint,
  lengthMin: number,
  zone: string | null,
): Plan {
  const start = addWall(
    { day: now.day, min: 0 },
    Math.min(snapMinutes(now.min, SNAP_MIN, "floor"), LAST_SLOT),
  );
  return finish(origin, { kind: "timed", start, end: addWall(start, lengthOf(lengthMin)) }, zone);
}

/** An edge dragged in its own piece's column (`pieceDay`; the pointer's column is ignored), pressed
 *  at `pressMinute` and now at `minute`: the edge goes as far as the pointer has, from wherever in
 *  the edge it was taken, so taking it changes nothing, and nothing changes until the pointer has
 *  gone half a step. It lands on the 15-minute grid: the end edge between a step after the start and
 *  that day's midnight, the start edge between that day's midnight and a step before the end. Wall
 *  minutes across days, so on an event of several days the end edge of its last piece can't pass
 *  the start. */
export function planResize(
  origin: TimedTiming,
  edge: "start" | "end",
  pieceDay: string,
  pressMinute: number,
  minute: number,
  zone: string | null,
): Plan {
  const travel = minute - pressMinute;
  if (Math.abs(travel) < SNAP_MIN / 2) return finish(origin, origin, zone);
  const midnight: WallPoint = { day: pieceDay, min: 0 };
  const from = wallDiff(midnight, edge === "end" ? origin.end : origin.start);
  const m = snapMinutes(clamp(from + travel, 0, DAY_MIN));
  if (edge === "end") {
    const earliest = addWall(origin.start, MIN_DURATION_MIN);
    const end = addWall(midnight, m);
    const timing: Timing = {
      kind: "timed",
      start: origin.start,
      end: wallDiff(earliest, end) < 0 ? earliest : end,
    };
    return finish(origin, timing, zone);
  }
  const latest = addWall(origin.end, -MIN_DURATION_MIN);
  const start = addWall(midnight, Math.min(m, LAST_SLOT));
  const timing: Timing = {
    kind: "timed",
    start: wallDiff(start, latest) < 0 ? latest : start,
    end: origin.end,
  };
  return finish(origin, timing, zone);
}

/** An all-day bar's end dragged to `day`; it never goes under one day. */
export function planBandResize(origin: AllDayTiming, edge: "left" | "right", day: string): Plan {
  const timing: Timing =
    edge === "right"
      ? { kind: "all_day", first: origin.first, last: day < origin.first ? origin.first : day }
      : { kind: "all_day", first: day > origin.last ? origin.last : day, last: origin.last };
  return finish(origin, timing, null);
}

/** A Month move: whole days, a timed event keeping its wall times. */
export function planMonthMove(
  origin: Timing,
  pressDay: string,
  day: string,
  zone: string | null,
): Plan {
  const n = daysBetween(pressDay, day);
  if (origin.kind === "all_day") {
    return finish(
      origin,
      { kind: "all_day", first: shiftDay(origin.first, n), last: shiftDay(origin.last, n) },
      null,
    );
  }
  const timing: Timing = {
    kind: "timed",
    start: { day: shiftDay(origin.start.day, n), min: origin.start.min },
    end: { day: shiftDay(origin.end.day, n), min: origin.end.min },
  };
  return finish(origin, timing, zone);
}

/** A drag down (or up) an empty column from `anchor`: the anchor's slot always, out to the pointer's
 *  slot, inside the anchor's day. */
export function planCreate(anchor: WallPoint, minute: number, zone: string | null): Plan {
  const a = Math.min(snapMinutes(anchor.min, SNAP_MIN, "floor"), LAST_SLOT);
  const m = clamp(minute, 0, DAY_MIN);
  const [from, to] =
    m >= a
      ? [a, Math.min(DAY_MIN, Math.max(a + SNAP_MIN, snapMinutes(m, SNAP_MIN, "ceil")))]
      : [Math.max(0, snapMinutes(m, SNAP_MIN, "floor")), a + SNAP_MIN];
  const midnight: WallPoint = { day: anchor.day, min: 0 };
  return finish(
    null,
    { kind: "timed", start: addWall(midnight, from), end: addWall(midnight, to) },
    zone,
  );
}

/** A click on an empty time: from its slot, `lengthMin` long; the end may run past midnight. */
export function planClickCreate(at: WallPoint, lengthMin: number, zone: string | null): Plan {
  const start = addWall(
    { day: at.day, min: 0 },
    Math.min(snapMinutes(at.min, SNAP_MIN, "floor"), LAST_SLOT),
  );
  return finish(null, { kind: "timed", start, end: addWall(start, lengthOf(lengthMin)) }, zone);
}

/** All day over two days, in order (the strip's and Month's create). */
export function planDayRange(anchorDay: string, day: string): Plan {
  const [first, last] = anchorDay <= day ? [anchorDay, day] : [day, anchorDay];
  return finish(null, { kind: "all_day", first, last }, null);
}

/** What a click on empty space at `at` creates, fixed at the press; null where it can't (no timed
 *  event without the device's zone). The view also passes null where creating isn't allowed. */
export function planClick(at: Target, opts: PlanOpts): Plan | null {
  if (at.zone !== "timed") return planDayRange(at.day, at.day);
  if (opts.zone === null) return null;
  return planClickCreate({ day: at.day, min: at.minute }, opts.lengthMin, opts.zone);
}

const OUTSIDE: Plan = { ok: false, why: "outside" };

/** The plan for this frame, dispatched on what was pressed and where the pointer is. Null keeps the
 *  last frame's plan: for an edge over the wrong kind of area (a timed edge over the strip, a strip
 *  end over the hours), and for a press that may do nothing, which so never has one. */
export function planGesture(press: Press, target: Target | null, opts: PlanOpts): Plan | null {
  if (!target) return OUTSIDE;
  const on = press.on;
  const at = press.at;
  if (on.kind === "empty") {
    if (on.create === null) return null;
    if (at.zone === "timed") {
      return target.zone === "timed"
        ? planCreate({ day: at.day, min: at.minute }, target.minute, opts.zone)
        : null;
    }
    return target.zone === at.zone ? planDayRange(at.day, target.day) : null;
  }
  const origin = on.origin;
  if (origin === null || !grabAllowed(on.grab, on.cap)) return null;
  if (on.surface === "month" || target.zone === "month") {
    return on.surface === "month" && target.zone === "month"
      ? planMonthMove(origin, at.day, target.day, opts.zone)
      : null;
  }
  switch (on.grab) {
    case "start":
    case "end":
      return origin.kind === "timed" && target.zone === "timed"
        ? planResize(
            origin,
            on.grab,
            on.pieceDay,
            at.zone === "timed" ? at.minute : target.minute,
            target.minute,
            opts.zone,
          )
        : null;
    case "left":
    case "right":
      return origin.kind === "all_day" && target.zone === "band"
        ? planBandResize(origin, on.grab, target.day)
        : null;
    case "body":
      if (origin.kind === "timed") {
        if (at.zone !== "timed") return null;
        const from: WallPoint = { day: at.day, min: at.minute };
        return target.zone === "timed"
          ? planTimedMove(origin, from, { day: target.day, min: target.minute }, opts.zone)
          : planTimedToBand(origin, from, target.day);
      }
      if (target.zone === "band") return planBandMove(origin, at.day, target.day);
      // Into the hours makes it a timed event, which needs the device's zone (R6).
      if (opts.zone === null) return OUTSIDE;
      return planBandToTimed(
        origin,
        { day: target.day, min: target.minute },
        opts.lengthMin,
        opts.zone,
      );
  }
}

// --- helpers ---------------------------------------------------------------------------------------

const pointOf = (d: Date): WallPoint => ({ day: dayKey(d), min: minutesFromLocalMidnight(d) });

/** A mirror row's time in the grid's terms, or null when it can't be moved: no end, or one before
 *  its start. An end at the next midnight is that day at 0; an all-day `last` is inclusive. */
export function timingOf(ev: CalendarEvent): Timing | null {
  if (!ev.end) return null;
  const start = parseLocal(ev.start, ev.all_day);
  const end = parseLocal(ev.end, ev.all_day);
  if (!start || !end) return null;
  if (ev.all_day) {
    const first = dayKey(start);
    const last = addDays(end, -1);
    return {
      kind: "all_day",
      first,
      last: last.getTime() < start.getTime() ? first : dayKey(last),
    };
  }
  if (end.getTime() < start.getTime()) return null;
  return { kind: "timed", start: pointOf(start), end: pointOf(end) };
}

/** Where a drop put an event, as the move sends it. */
export function toMoveTo(t: Timing): MoveTo {
  if (t.kind === "all_day") return { kind: "all_day", first_day: t.first, last_day: t.last };
  return {
    kind: "timed",
    start_date: t.start.day,
    start_time: clockOf(t.start.min),
    end_date: t.end.day,
    end_time: clockOf(t.end.min),
  };
}

/** The dragged row's time exactly as it holds it: what the move checks Google still has. */
export function seenTimesOf(ev: CalendarEvent): SeenTimes {
  return { start: ev.start, end: ev.end ?? null, all_day: ev.all_day };
}

/** A wall point as the local instant the grid would draw there. Built like the grid's own days, so it
 *  never fails: a time the clocks skip comes out an hour on, where the browser puts it. */
function localIso(p: WallPoint): string {
  const [y, m, d] = p.day.split("-").map(Number);
  const at = new Date(y, m - 1, d, Math.floor(p.min / 60), p.min % 60);
  // Whole minutes, as the mirror writes them.
  return `${at.toISOString().slice(0, 19)}Z`;
}

/** `origin` drawn at `t`: start and end in the mirror's shapes (a UTC `…Z` instant, or a date with
 *  the exclusive end), every other field the origin's, the same id. */
export function ghostEvent(origin: CalendarEvent, t: Timing): CalendarEvent {
  if (t.kind === "all_day") {
    return { ...origin, start: t.first, end: shiftDay(t.last, 1), all_day: true };
  }
  return { ...origin, start: localIso(t.start), end: localIso(t.end), all_day: false };
}

/** The first half of `t` that changed from `origin` and is a time the clocks skip in `zone`, or null.
 *  A half left where it was exists, so it's never checked. */
export function gapIn(origin: Timing | null, t: Timing, zone: string): WallPoint | null {
  if (t.kind !== "timed") return null;
  for (const half of ["start", "end"] as const) {
    const p = t[half];
    if (origin?.kind === "timed" && samePoint(origin[half], p)) continue;
    if (resolveWallTime(p.day, clockOf(p.min), zone).kind === "gap") return p;
  }
  return null;
}

/** The minutes `t` fills on `day`, or null when it isn't on that day. */
function pieceOn(t: TimedTiming, day: string): { from: number; to: number } | null {
  const midnight: WallPoint = { day, min: 0 };
  const from = Math.max(0, wallDiff(midnight, t.start));
  const to = Math.min(DAY_MIN, wallDiff(midnight, t.end));
  return from < DAY_MIN && to > 0 && to >= from ? { from, to } : null;
}

/** Where a slot sits on screen now: its first visible piece in the time grid, its first day's slice
 *  of the strip, or its first Month cell. Null when none of it is on screen. Quick create anchors
 *  here. */
export function slotRect(g: TimeGridGeom | MonthGeom, t: Timing): Box | null {
  if ("weekH" in g) return monthSlot(g, t);
  if (t.kind === "all_day") {
    const band = g.band;
    if (!band || band.days.length === 0) return null;
    const i = band.days.findIndex((d) => d >= t.first && d <= t.last);
    if (i < 0) return null;
    const w = (band.right - band.left) / band.days.length;
    return {
      left: band.left + i * w,
      right: band.left + (i + 1) * w,
      top: band.top,
      bottom: band.bottom,
    };
  }
  for (const col of g.cols) {
    const piece = pieceOn(t, col.day);
    if (!piece) continue;
    const top = g.body.top - g.scrollTop + (piece.from / 60) * g.rowH;
    const bottom = g.body.top - g.scrollTop + (piece.to / 60) * g.rowH;
    if (bottom <= g.body.top || top >= g.body.bottom) continue; // scrolled out of the body
    return { left: col.left, right: col.right, top, bottom };
  }
  return null;
}

function monthSlot(g: MonthGeom, t: Timing): Box | null {
  if (g.weeks <= 0 || !(g.weekH > 0)) return null;
  const first = daysBetween(g.gridStart, t.kind === "all_day" ? t.first : t.start.day);
  const last = daysBetween(g.gridStart, t.kind === "all_day" ? t.last : lastDayOf(t));
  const w = (g.stream.right - g.stream.left) / 7;
  // The first cell of it from the top row on screen, if that's still on screen.
  const topRow = Math.max(0, Math.floor(g.scrollTop / g.weekH));
  const i = Math.max(first, topRow * 7, 0);
  if (i > Math.min(last, g.weeks * 7 - 1)) return null;
  const row = Math.floor(i / 7);
  const col = i % 7;
  const top = g.stream.top - g.scrollTop + row * g.weekH;
  if (top >= g.stream.bottom) return null;
  return {
    left: g.stream.left + col * w,
    right: g.stream.left + (col + 1) * w,
    top,
    bottom: top + g.weekH,
  };
}

/** What may change an event's time, from the view (`eventRights`, C16a). */
export interface CapabilityContext {
  rights: { canRetime: boolean; reason: string | null };
  /** A move of it is being asked about or saved. */
  pending: boolean;
  /** The device's zone is known (a timed event can't be moved without it). */
  zoneKnown: boolean;
}

const nothing = (reason: string | null): DragCapability => ({
  move: false,
  resizeStart: false,
  resizeEnd: false,
  bandEdges: false,
  reason,
});

/** What a press on `ev` may do, and, when nothing, why (the first that applies). */
export function dragCapability(ev: CalendarEvent, ctx: CapabilityContext): DragCapability {
  // A milestone, a pinboard entry or the new event's draft: no drag, and nothing to explain.
  if (isOverlayEvent(ev) || isDraftEvent(ev)) return nothing(null);
  if (ctx.pending) return nothing(SAVING);
  if (!ctx.rights.canRetime) return nothing(ctx.rights.reason);
  const t = timingOf(ev);
  if (!t) return nothing(NO_END);
  if (t.kind === "timed") {
    if (!ctx.zoneKnown) return nothing(NO_ZONE);
    return { move: true, resizeStart: true, resizeEnd: true, bandEdges: false, reason: null };
  }
  return { move: true, resizeStart: false, resizeEnd: false, bandEdges: true, reason: null };
}

/** What a drop of `ev` asks before it's sent: which occurrences (a repeating event), and whether
 *  to email its guests (`guests`, the row's notice for a move; null when nobody). */
export function dropChoicesNeeded(
  ev: CalendarEvent,
  guests: GuestNotice | null,
): { scope: boolean; guests: GuestNotice | null } {
  return { scope: isRepeating(ev), guests };
}

/** A move shown at its new place before the calendar's copy has it (the pending look). */
export interface PendingMove {
  eventId: string;
  timing: Timing;
  /** asking: a dialog is open about it; saving: sent; landed: Google took it, shown until a read
   *  that has it. */
  phase: "asking" | "saving" | "landed";
  /** landed: the write count it landed at; a read that started at or after it has Google's copy.
   *  Null when Google took it but PM's copy wasn't refreshed: then only a read that shows the new
   *  time lets it go, or `until`. */
  mark: number | null;
  /** When a landed entry with no mark is let go anyway (ms since the epoch); null otherwise. */
  until: number | null;
  /** asking: the token of the dialogs asking about it; the entry goes when they do (one with none
   *  goes on the next read). */
  flow: string | null;
  /** The occurrence key it had before, so another account's copy of it can be hidden. */
  twinKey: string | null;
}

/** `events` with each pending move drawn at its new time; every other row is the same object. */
export function applyPending(
  events: readonly CalendarEvent[],
  pending: ReadonlyMap<string, PendingMove>,
): CalendarEvent[] {
  return events.map((ev) => {
    const p = pending.get(ev.id);
    return p ? ghostEvent(ev, p.timing) : ev;
  });
}

/** The pending moves still to show after a read of the mirror that started at `startedAt` (a read
 *  mark) found `events`. One goes when the read lacks its row (moving every occurrence changes their
 *  ids; a row deleted elsewhere), when the dialogs asking about it are gone (`flows` holds those
 *  open), or when it landed and the read has Google's copy: by its mark, or, with none, by showing
 *  the new time or reaching `until`. One being asked about or saved otherwise stays, whatever the
 *  mark. The same map when nothing went. */
export function prunePending(
  pending: ReadonlyMap<string, PendingMove>,
  events: readonly CalendarEvent[],
  startedAt: number,
  live: { flows: ReadonlySet<string>; now: number },
): ReadonlyMap<string, PendingMove> {
  if (pending.size === 0) return pending;
  const rows = new Map(events.map((e) => [e.id, e]));
  const keep = (p: PendingMove): boolean => {
    const row = rows.get(p.eventId);
    if (!row) return false;
    if (p.phase === "asking") return p.flow !== null && live.flows.has(p.flow);
    if (p.phase === "saving") return true;
    if (p.mark !== null) return p.mark > startedAt;
    const shown = timingOf(row);
    if (shown && isNoop(shown, p.timing)) return false;
    return p.until === null || live.now < p.until;
  };
  const still = [...pending].filter(([, p]) => keep(p));
  return still.length === pending.size ? pending : new Map(still);
}
