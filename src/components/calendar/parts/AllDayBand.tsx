// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The all-day / multi-day strip above the time-grid body. Each band is packed into a stacked lane
// (calendar-layout.packBands) and positioned by day index across the visible columns, clamped to the
// window with flat edges where it continues off-screen. Fill/border are the per-source colour mixed
// into transparency via color-mix (token-safe — no source hex is written here). Renders nothing when
// no band events fall in range, so the row collapses.

import { useMemo } from "react";
import type { CalendarEvent } from "../../../lib/types";
import {
  clampSpanToRange,
  dayDiff,
  eventDaySpan,
  packBands,
  type BandInput,
} from "../../../lib/calendar-layout";

interface Props {
  /** Band events only (multi-day, or single all-day) — the view pre-filters. */
  events: CalendarEvent[];
  /** The visible day columns (local midnights), left → right. */
  days: Date[];
  colorOf: (calendarId: string) => string;
  /** Left gutter width, matched to the time-grid's hour gutter so bands align with columns. */
  gutterPx: number;
  /** Depth gate for the "all-day" gutter label. */
  showLabel: boolean;
}

const LANE_H = 20;

interface Placed extends BandInput {
  ev: CalendarEvent;
  continuesLeft: boolean;
  continuesRight: boolean;
}

export function AllDayBand({ events, days, colorOf, gutterPx, showLabel }: Props) {
  const ndays = days.length;

  const { placed, laneCount } = useMemo(() => {
    if (ndays === 0) return { placed: [] as (Placed & { lane: number })[], laneCount: 0 };
    const anchor = days[0];
    const lastIndex = ndays - 1;
    const inputs: Placed[] = [];
    for (const ev of events) {
      const span = eventDaySpan(ev);
      if (!span) continue;
      const clamped = clampSpanToRange(
        dayDiff(anchor, span.startDay),
        dayDiff(anchor, span.endDay),
        lastIndex,
      );
      if (!clamped) continue;
      inputs.push({
        id: ev.id,
        startDay: clamped.startDay,
        endDay: clamped.endDay,
        continuesLeft: clamped.continuesLeft,
        continuesRight: clamped.continuesRight,
        ev,
      });
    }
    const { bands, laneCount } = packBands(inputs);
    const byId = new Map(inputs.map((i) => [i.id, i]));
    const placed = bands.map((b) => {
      const src = byId.get(b.id)!;
      return { ...src, lane: b.lane };
    });
    return { placed, laneCount };
  }, [events, days, ndays]);

  if (laneCount === 0) return null;

  return (
    <div className="flex border-b border-rule">
      <div
        className="flex shrink-0 items-start justify-end pr-2 pt-1"
        style={{ width: `${gutterPx}px` }}
      >
        {showLabel && <span className="font-mono text-[9px] text-faint">all-day</span>}
      </div>
      <div className="relative flex-1" style={{ height: `${laneCount * LANE_H}px` }}>
        {placed.map((b) => {
          const color = colorOf(b.ev.calendar_id);
          const leftPct = (b.startDay / ndays) * 100;
          const widthPct = ((b.endDay - b.startDay + 1) / ndays) * 100;
          return (
            <div
              key={b.ev.id}
              className="absolute overflow-hidden px-1.5 text-[11px] leading-[18px]"
              style={{
                top: `${b.lane * LANE_H}px`,
                left: `${leftPct}%`,
                width: `${widthPct}%`,
                height: `${LANE_H - 2}px`,
                background: `color-mix(in oklab, ${color} 22%, transparent)`,
                borderLeft: b.continuesLeft ? undefined : `3px solid ${color}`,
                borderTopLeftRadius: b.continuesLeft ? 0 : "var(--radius-sm)",
                borderBottomLeftRadius: b.continuesLeft ? 0 : "var(--radius-sm)",
                borderTopRightRadius: b.continuesRight ? 0 : "var(--radius-sm)",
                borderBottomRightRadius: b.continuesRight ? 0 : "var(--radius-sm)",
              }}
              title={b.ev.summary}
              aria-label={b.ev.summary}
            >
              <span className="truncate font-head text-ink">
                {b.continuesLeft ? "‹ " : ""}
                {b.ev.summary}
              </span>
            </div>
          );
        })}
      </div>
    </div>
  );
}
