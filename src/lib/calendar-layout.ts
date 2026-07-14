// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Pure, DOM-free calendar geometry for the unified view (card 8): local-day math, event → day-span
// resolution, timed-event overlap lanes, and multi-day band packing. Kept free of React and the DOM
// so the fiddly parts (all-day exclusive ends, midnight/DST boundaries, overlap clustering) are
// deterministic and testable in isolation; the view components only position what these return.

import { pad2 } from "./format";
import type { CalendarEvent } from "./types";

// --- milestone overlay -----------------------------------------------------------------------------

/** The synthetic `calendar_id` carried by project-milestone events (card 7 overlay). These are
 *  injected by CalendarView, not synced — they render as all-day events but are the one clickable,
 *  navigational element on the otherwise read-only calendar. Their `id` is `milestone:<milestone id>`. */
export const MILESTONE_CALENDAR_ID = "pm:milestones";

/** True for a synthetic project-milestone event (vs a real synced event). */
export function isMilestoneEvent(ev: CalendarEvent): boolean {
  return ev.calendar_id === MILESTONE_CALENDAR_ID;
}

/** The pseudo-calendar carrying the pinboard overlay: dated entries from freeform (not-yet-linked)
 *  timeline widgets. Toggled like a calendar via the same `hidden` set. */
export const PINBOARD_CALENDAR_ID = "pm:pinboard";

/** True for a synthetic pinboard-timeline event (vs a real synced event). */
export function isPinboardEvent(ev: CalendarEvent): boolean {
  return ev.calendar_id === PINBOARD_CALENDAR_ID;
}

/** True for any first-party PM overlay event (a milestone or a pinboard entry). These are the only
 *  clickable things on the otherwise read-only calendar — a synced event is never wired to a click. */
export function isOverlayEvent(ev: CalendarEvent): boolean {
  return isMilestoneEvent(ev) || isPinboardEvent(ev);
}

// --- local-day helpers ---------------------------------------------------------------------------

/** Parse a mirror date string to a LOCAL `Date`. An all-day value is a civil date ("YYYY-MM-DD")
 *  with no zone, so it MUST be read as local midnight — `new Date("2026-07-01")` parses as UTC and
 *  renders a day early west of UTC. A timed value carries a zone (…Z), where `new Date` is correct. */
export function parseLocal(value: string, allDay: boolean): Date | null {
  if (allDay || !value.includes("T")) {
    const m = /^(\d{4})-(\d{2})-(\d{2})/.exec(value);
    if (!m) return null;
    return new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
  }
  const d = new Date(value);
  return Number.isNaN(d.getTime()) ? null : d;
}

/** Local midnight of `d`'s calendar day. */
export function startOfDay(d: Date): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate());
}

/** `d` shifted by `n` whole calendar days (DST-safe — built from y/m/d, not a ms offset). */
export function addDays(d: Date, n: number): Date {
  return new Date(d.getFullYear(), d.getMonth(), d.getDate() + n);
}

/** Local `YYYY-MM-DD` key for bucketing events into days. */
export function dayKey(d: Date): string {
  return `${d.getFullYear()}-${pad2(d.getMonth() + 1)}-${pad2(d.getDate())}`;
}

/** Whole-day difference `b - a` in local calendar days (rounding absorbs the 23/25h DST days). */
export function dayDiff(a: Date, b: Date): number {
  return Math.round((startOfDay(b).getTime() - startOfDay(a).getTime()) / 86_400_000);
}

/** Minutes since local midnight (0..1440). */
export function minutesFromLocalMidnight(d: Date): number {
  return d.getHours() * 60 + d.getMinutes();
}

/** The minute-of-day (0..1440) a timed event's span should END at on the day grid. An event ending at
 *  exactly 00:00 the next day ("…until midnight") must reach 1440 — the bottom of the grid day — not 0,
 *  which collapses it to a 14px sliver (F-62). Mirrors the exclusive-midnight rule `eventDaySpan` uses.
 *  A null `end` yields a default 30-minute block from `start`. */
export function timedEndMinutes(start: Date, end: Date | null): number {
  if (!end) return minutesFromLocalMidnight(start) + 30;
  const endMin = minutesFromLocalMidnight(end);
  return endMin === 0 && end.getTime() > start.getTime() ? 1440 : endMin;
}

// --- event → day span ----------------------------------------------------------------------------

/** An event's inclusive local-day span. */
export interface DaySpan {
  startDay: Date;
  endDay: Date;
}

/** The inclusive local-day span an event occupies. All-day `end` is EXCLUSIVE (a one-day all-day
 *  event is start=D, end=D+1), so the last occupied day is `end − 1`, floored at the start day. A
 *  timed event spans the local days its start..end touch. Returns null if the start is unparseable. */
export function eventDaySpan(ev: CalendarEvent): DaySpan | null {
  const start = parseLocal(ev.start, ev.all_day);
  if (!start) return null;
  const startDay = startOfDay(start);
  let endDay = startDay;
  if (ev.end) {
    const end = parseLocal(ev.end, ev.all_day);
    if (end) {
      if (ev.all_day) {
        const lastInclusive = addDays(startOfDay(end), -1);
        endDay = lastInclusive.getTime() >= startDay.getTime() ? lastInclusive : startDay;
      } else if (minutesFromLocalMidnight(end) === 0 && end.getTime() > start.getTime()) {
        // A timed event ending exactly at 00:00 the next day (a normal provider way to say "…until
        // midnight") occupies up to the previous day, not into that next day — mirror the all-day
        // exclusive-end rule so it isn't misclassified as a 2-day band. Floored at the start day.
        const lastInclusive = addDays(startOfDay(end), -1);
        endDay = lastInclusive.getTime() >= startDay.getTime() ? lastInclusive : startDay;
      } else {
        endDay = startOfDay(end);
      }
    }
  }
  return { startDay, endDay };
}

/** Locale short weekday name ("Mon"), shared by both agendas so the day header never drifts. */
export function weekdayShort(d: Date): string {
  return d.toLocaleDateString(undefined, { weekday: "short" });
}

/** Ordering for a day's event list: all-day events first, then by start instant (ISO strings sort
 *  chronologically). Shared by the month grids and both agendas so the sort never drifts. */
export function compareEventsForDay(a: CalendarEvent, b: CalendarEvent): number {
  if (a.all_day !== b.all_day) return a.all_day ? -1 : 1;
  return String(a.start).localeCompare(String(b.start));
}

/** A day and the events that appear on it. */
export interface DayGroup {
  day: Date;
  items: CalendarEvent[];
}

/** Group events for an open-ended agenda from `fromDay` forward, one bucket per day (empty days
 *  omitted), groups and items sorted. An in-progress multi-day event whose run started BEFORE the
 *  anchor but still reaches it is kept and bucketed under `fromDay` (not silently dropped). */
export function groupEventsFromDay(events: CalendarEvent[], fromDay: Date): DayGroup[] {
  const fromMs = startOfDay(fromDay).getTime();
  const byDay = new Map<string, DayGroup>();
  for (const ev of events) {
    const span = eventDaySpan(ev);
    if (!span) continue;
    // Keep the event unless the WHOLE thing ended before the anchor. Bucket it under the later of
    // its start day and the anchor, so a still-running multi-day event surfaces on the anchor day.
    if (span.endDay.getTime() < fromMs) continue;
    const bucket = startOfDay(new Date(Math.max(span.startDay.getTime(), fromMs)));
    const key = dayKey(bucket);
    const g = byDay.get(key);
    if (g) g.items.push(ev);
    else byDay.set(key, { day: bucket, items: [ev] });
  }
  const ordered = [...byDay.values()].sort((a, b) => a.day.getTime() - b.day.getTime());
  for (const g of ordered) g.items.sort(compareEventsForDay);
  return ordered;
}

/** True when an event covers more than one local day (a multi-day all-day event, or a timed event
 *  crossing midnight / longer than a day) — so it belongs in the all-day band, not a time column. */
export function isMultiDay(ev: CalendarEvent): boolean {
  const span = eventDaySpan(ev);
  return !!span && span.endDay.getTime() > span.startDay.getTime();
}

// --- timed-event overlap lanes -------------------------------------------------------------------

export interface TimedInput {
  id: string;
  startMin: number;
  endMin: number;
}

export interface LaneInfo {
  lane: number;
  lanes: number;
}

/** Assign each timed event a column lane within its overlap cluster: sort by start, sweep into
 *  maximal transitively-overlapping clusters (tracking the running max end), and give each event the
 *  lowest free lane. Every event in a cluster shares the cluster's lane count so columns line up.
 *  A zero/negative duration is treated as a 1-minute sliver so it still gets a lane. Pure, O(n log n). */
export function layoutLanes(events: TimedInput[]): Map<string, LaneInfo> {
  const MIN = 1;
  const evs = events
    .map((e) => ({ id: e.id, startMin: e.startMin, endMin: Math.max(e.endMin, e.startMin + MIN) }))
    .sort((a, b) => a.startMin - b.startMin || b.endMin - a.endMin);

  const out = new Map<string, LaneInfo>();
  let cluster: { id: string; startMin: number; endMin: number }[] = [];
  let clusterEnd = -Infinity;

  const flush = () => {
    if (cluster.length === 0) return;
    const laneEnds: number[] = [];
    const laneOf = new Map<string, number>();
    for (const e of cluster) {
      let lane = laneEnds.findIndex((end) => end <= e.startMin);
      if (lane === -1) {
        lane = laneEnds.length;
        laneEnds.push(e.endMin);
      } else {
        laneEnds[lane] = e.endMin;
      }
      laneOf.set(e.id, lane);
    }
    const lanes = laneEnds.length;
    for (const e of cluster) out.set(e.id, { lane: laneOf.get(e.id) ?? 0, lanes });
    cluster = [];
    clusterEnd = -Infinity;
  };

  for (const e of evs) {
    if (cluster.length > 0 && e.startMin >= clusterEnd) flush();
    cluster.push(e);
    clusterEnd = Math.max(clusterEnd, e.endMin);
  }
  flush();
  return out;
}

export interface ColumnPlacement {
  id: string;
  lane: number;
  lanes: number;
}

/** Equal-width columns: each event gets `1/lanes` width at `lane/lanes`. */
export function assignColumns(events: TimedInput[]): ColumnPlacement[] {
  const lanes = layoutLanes(events);
  return events.map((e) => {
    const info = lanes.get(e.id) ?? { lane: 0, lanes: 1 };
    return { id: e.id, lane: info.lane, lanes: info.lanes };
  });
}

// --- multi-day / all-day band packing ------------------------------------------------------------

export interface BandInput {
  id: string;
  /** Inclusive day indices within the visible range. */
  startDay: number;
  endDay: number;
}

export interface BandPlacement extends BandInput {
  lane: number;
}

/** Greedy lane-pack multi-day/all-day spans into stacked rows: sort by start then longest-first
 *  (longer bands take lower lanes for stable stacking), and place each in the lowest lane free before
 *  it starts. Pure. */
export function packBands(spans: BandInput[]): { bands: BandPlacement[]; laneCount: number } {
  const sorted = [...spans].sort(
    (a, b) => a.startDay - b.startDay || b.endDay - b.startDay - (a.endDay - a.startDay),
  );
  const laneLastDay: number[] = [];
  const bands: BandPlacement[] = [];
  for (const s of sorted) {
    let lane = laneLastDay.findIndex((last) => last < s.startDay);
    if (lane === -1) {
      lane = laneLastDay.length;
      laneLastDay.push(s.endDay);
    } else {
      laneLastDay[lane] = s.endDay;
    }
    bands.push({ id: s.id, startDay: s.startDay, endDay: s.endDay, lane });
  }
  return { bands, laneCount: laneLastDay.length };
}

export interface ClampedSpan {
  startDay: number;
  endDay: number;
  continuesLeft: boolean;
  continuesRight: boolean;
}

/** Clamp an inclusive `[startDay, endDay]` span to `[0, lastIndex]`, flagging where it was cut so the
 *  band renders a flat edge / continuation arrow. Returns null when the span misses the range. */
export function clampSpanToRange(
  startDay: number,
  endDay: number,
  lastIndex: number,
): ClampedSpan | null {
  if (endDay < 0 || startDay > lastIndex) return null;
  return {
    startDay: Math.max(0, startDay),
    endDay: Math.min(lastIndex, endDay),
    continuesLeft: startDay < 0,
    continuesRight: endDay > lastIndex,
  };
}
