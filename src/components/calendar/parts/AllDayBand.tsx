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
  isEventPast,
  packBands,
  PAST_EVENT_CLASS,
  type BandInput,
} from "../../../lib/calendar-layout";
import { cn } from "../../ui";

interface Props {
  /** Band events only (multi-day, or single all-day) — the view pre-filters. */
  events: CalendarEvent[];
  /** The visible day columns (local midnights), left → right. */
  days: Date[];
  colorOf: (calendarId: string) => string;
  /** Left gutter width, matched to the time-grid's hour gutter so bands align with columns. */
  gutterPx: number;
  /** Right gutter matching the scrolling body's vertical-scrollbar width, so the band's columns line
   *  up with the time-grid below it (0 under overlay scrollbars). */
  endGutterPx?: number;
  /** Depth gate for the "all-day" gutter label. */
  showLabel: boolean;
  /** The ticking "now" (device-local) so a band whose last day is past greys back. Defaults to the
   *  render-time clock for embeds that don't thread a tick. */
  now?: Date;
  /** Open an event's detail popup, anchored at the band's on-screen rect. */
  onEventClick?: (ev: CalendarEvent, anchor: DOMRect) => void;
}

const LANE_H = 20;

interface Placed extends BandInput {
  ev: CalendarEvent;
  continuesLeft: boolean;
  continuesRight: boolean;
}

export function AllDayBand({
  events,
  days,
  colorOf,
  gutterPx,
  endGutterPx = 0,
  showLabel,
  now,
  onEventClick,
}: Props) {
  const ndays = days.length;
  const nowRef = now ?? new Date();

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
    <div className="flex border-b border-rule" style={{ paddingRight: endGutterPx }}>
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
          const clickable = !!onEventClick;
          const past = isEventPast(b.ev, nowRef);
          return (
            <div
              key={b.ev.id}
              className={cn(
                "absolute overflow-hidden px-1.5 text-[11px] leading-[18px]",
                clickable && "cursor-pointer hover:brightness-110",
                past && PAST_EVENT_CLASS,
              )}
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
              role={clickable ? "button" : undefined}
              tabIndex={clickable ? 0 : undefined}
              onClick={
                clickable
                  ? (e) => onEventClick?.(b.ev, e.currentTarget.getBoundingClientRect())
                  : undefined
              }
              onKeyDown={
                clickable
                  ? (e) => {
                      if (e.key === "Enter" || e.key === " ") {
                        e.preventDefault();
                        onEventClick?.(b.ev, e.currentTarget.getBoundingClientRect());
                      }
                    }
                  : undefined
              }
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
