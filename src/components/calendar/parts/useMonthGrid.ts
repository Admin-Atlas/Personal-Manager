// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The pure month-grid computation shared by the Slate/Editorial MonthView and the Terminal
// MonthTable — a Monday-first 6×7 grid with single-day events bucketed as chips and multi-day events
// packed into stacked bands. Extracted so the two Systems can't drift (a band-lane or overflow fix
// lands once). Multi-day bands are packed ONCE across the whole grid, so a run keeps the same lane
// row across week boundaries; bands past the lane cap are counted per covered day so the view can
// surface a "+N more" instead of silently dropping them.

import { useMemo } from "react";
import type { CalendarEvent } from "../../../lib/types";
import {
  addDays,
  clampSpanToRange,
  compareEventsForDay,
  dayDiff,
  dayKey,
  eventDaySpan,
  isMultiDay,
  packBands,
  parseLocal,
  startOfDay,
  type BandInput,
} from "../../../lib/calendar-layout";

/** How many band lanes a month cell shows before extra bands fold into the "+N more" count. */
export const MONTH_MAX_BAND_LANES = 3;

export interface MonthDayCell {
  date: Date;
  inMonth: boolean;
  isToday: boolean;
  /** Single-day events on this day, all-day first. */
  chips: CalendarEvent[];
  /** Multi-day bands that cover this day but were dropped past the lane cap (surface in overflow). */
  hiddenBands: number;
}

export interface MonthBandBar {
  ev: CalendarEvent;
  /** Column indices within the week (0..6). */
  startIdx: number;
  endIdx: number;
  lane: number;
  continuesLeft: boolean;
  continuesRight: boolean;
}

export interface MonthWeekRow {
  cells: MonthDayCell[];
  bands: MonthBandBar[];
  /** Lanes to reserve height for in this week (max shown lane + 1). */
  laneCount: number;
}

export interface MonthGrid {
  weeks: MonthWeekRow[];
  weekdayLabels: string[];
}

function buildMonthGrid(cursor: Date, events: CalendarEvent[]): MonthGrid {
  const year = cursor.getFullYear();
  const month = cursor.getMonth();
  const first = new Date(year, month, 1);
  const lead = (first.getDay() + 6) % 7; // Monday-first offset
  const gridStart = addDays(startOfDay(first), -lead);
  const daysInMonth = new Date(year, month + 1, 0).getDate();
  const rows = Math.ceil((lead + daysInMonth) / 7);
  const lastIdx = rows * 7 - 1;
  const todayKey = dayKey(startOfDay(new Date()));

  // Single-day events → chips bucketed by local start day.
  const chipsByDay = new Map<string, CalendarEvent[]>();
  const bandEvents: CalendarEvent[] = [];
  for (const ev of events) {
    if (isMultiDay(ev)) {
      bandEvents.push(ev);
      continue;
    }
    const start = parseLocal(ev.start, ev.all_day);
    if (!start) continue;
    const key = dayKey(startOfDay(start));
    const list = chipsByDay.get(key);
    if (list) list.push(ev);
    else chipsByDay.set(key, [ev]);
  }
  for (const list of chipsByDay.values()) list.sort(compareEventsForDay);

  // Pack every multi-day band ONCE across the whole visible grid so each event keeps a stable lane
  // across weeks (per-week packing let a run jump rows at the week boundary).
  const bandInputs: BandInput[] = [];
  const bandMeta = new Map<string, { ev: CalendarEvent; gStart: number; gEnd: number }>();
  for (const ev of bandEvents) {
    const span = eventDaySpan(ev);
    if (!span) continue;
    const clamped = clampSpanToRange(
      dayDiff(gridStart, span.startDay),
      dayDiff(gridStart, span.endDay),
      lastIdx,
    );
    if (!clamped) continue;
    bandInputs.push({ id: ev.id, startDay: clamped.startDay, endDay: clamped.endDay });
    bandMeta.set(ev.id, { ev, gStart: clamped.startDay, gEnd: clamped.endDay });
  }
  const { bands: globalBands } = packBands(bandInputs);
  const laneOf = new Map(globalBands.map((b) => [b.id, b.lane]));

  const weeks: MonthWeekRow[] = [];
  for (let r = 0; r < rows; r++) {
    const weekStartIdx = r * 7;
    const weekEndIdx = weekStartIdx + 6;
    const weekStart = addDays(gridStart, weekStartIdx);
    const hiddenByCol = new Array(7).fill(0);
    const bars: MonthBandBar[] = [];
    let laneCount = 0;

    for (const [id, { ev, gStart, gEnd }] of bandMeta) {
      if (gEnd < weekStartIdx || gStart > weekEndIdx) continue; // band doesn't touch this week
      const lane = laneOf.get(id) ?? 0;
      const localStart = Math.max(gStart, weekStartIdx) - weekStartIdx;
      const localEnd = Math.min(gEnd, weekEndIdx) - weekStartIdx;
      if (lane < MONTH_MAX_BAND_LANES) {
        laneCount = Math.max(laneCount, lane + 1);
        bars.push({
          ev,
          startIdx: localStart,
          endIdx: localEnd,
          lane,
          continuesLeft: gStart < weekStartIdx,
          continuesRight: gEnd > weekEndIdx,
        });
      } else {
        for (let c = localStart; c <= localEnd; c++) hiddenByCol[c]++;
      }
    }

    const cells: MonthDayCell[] = Array.from({ length: 7 }, (_, i) => {
      const date = addDays(weekStart, i);
      return {
        date,
        inMonth: date.getMonth() === month,
        isToday: dayKey(date) === todayKey,
        chips: chipsByDay.get(dayKey(date)) ?? [],
        hiddenBands: hiddenByCol[i],
      };
    });
    weeks.push({ cells, bands: bars, laneCount });
  }

  const weekdayLabels = Array.from({ length: 7 }, (_, i) =>
    addDays(gridStart, i).toLocaleDateString(undefined, { weekday: "short" }),
  );
  return { weeks, weekdayLabels };
}

/** Memoised {@link buildMonthGrid} for a component. */
export function useMonthGrid(cursor: Date, events: CalendarEvent[]): MonthGrid {
  return useMemo(() => buildMonthGrid(cursor, events), [cursor, events]);
}
