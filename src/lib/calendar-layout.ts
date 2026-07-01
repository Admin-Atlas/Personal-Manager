// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Pure, DOM-free calendar geometry for the unified view (card 8): local-day math, event → day-span
// resolution, timed-event overlap lanes, and multi-day band packing. Kept free of React and the DOM
// so the fiddly parts (all-day exclusive ends, midnight/DST boundaries, overlap clustering) are
// deterministic and testable in isolation; the view components only position what these return.

import type { CalendarEvent } from "./types";

function pad2(n: number): string {
  return String(n).padStart(2, "0");
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

/** Whole local days in `[start, end]` inclusive (by calendar day). Bounded against bad data. */
export function eachDayInRange(start: Date, end: Date): Date[] {
  const out: Date[] = [];
  let cur = startOfDay(start);
  const last = startOfDay(end).getTime();
  let guard = 0;
  while (cur.getTime() <= last && guard < 2000) {
    out.push(cur);
    cur = addDays(cur, 1);
    guard++;
  }
  return out;
}

/** Whole-day difference `b - a` in local calendar days (rounding absorbs the 23/25h DST days). */
export function dayDiff(a: Date, b: Date): number {
  return Math.round((startOfDay(b).getTime() - startOfDay(a).getTime()) / 86_400_000);
}

/** Minutes since local midnight (0..1440). */
export function minutesFromLocalMidnight(d: Date): number {
  return d.getHours() * 60 + d.getMinutes();
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
      } else {
        endDay = startOfDay(end);
      }
    }
  }
  return { startDay, endDay };
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

export interface CascadePlacement {
  id: string;
  lane: number;
  clusterSize: number;
}

/** Cascade: same lane data as columns, but later lanes inset + stack (rendered full-width-minus-inset
 *  with a drop shadow, higher lane on top). */
export function assignCascade(events: TimedInput[]): CascadePlacement[] {
  const lanes = layoutLanes(events);
  return events.map((e) => {
    const info = lanes.get(e.id) ?? { lane: 0, lanes: 1 };
    return { id: e.id, lane: info.lane, clusterSize: info.lanes };
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
