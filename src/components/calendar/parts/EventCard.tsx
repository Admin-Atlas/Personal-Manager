// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// One timed event in the pixel time-grid (Week/Day). Absolutely positioned by the geometry the view
// computes from calendar-layout; the card itself only paints. The fill is a shade of the event's own
// source colour mixed into the surface via color-mix (token-safe — no source hex written here), so a
// timed block reads as its calendar's colour like the all-day bands do, instead of a flat neutral that
// vanished into the grid. Opaque-over-surface (not …, transparent) on purpose: timed cards overlap and
// sit over the today-column tint, where a translucent fill goes muddy. Meta reveals with depth AND
// available height (a 20px sliver has no room for a time line); a past event is greyed via isPast.

import type { CSSProperties } from "react";
import { cn } from "../../ui";
import { PAST_EVENT_CLASS } from "../../../lib/calendar-layout";

interface Props {
  summary: string;
  /** The calendar's source colour (categorical palette) — the left rule. */
  color: string;
  /** Local clock label, e.g. "09:30". */
  timeLabel: string;
  location: string | null;
  /** Absolute geometry within the day column. */
  topPx: number;
  heightPx: number;
  leftPct: number;
  widthPct: number;
  /** Depth gate for the time line (also needs a tall-enough card). */
  showTime: boolean;
  /** Depth gate for the location line (Power, and only on tall cards). */
  showLocation: boolean;
  /** The event has fully passed — grey it back so what's done recedes. */
  isPast?: boolean;
}

// Height thresholds below which a line has no room — keep them out of the render so a squeezed card
// stays just its title rather than overflowing.
const TIME_MIN_H = 38;
const LOC_MIN_H = 52;

export function EventCard({
  summary,
  color,
  timeLabel,
  location,
  topPx,
  heightPx,
  leftPct,
  widthPct,
  showTime,
  showLocation,
  isPast,
}: Props) {
  const style: CSSProperties = {
    top: `${topPx}px`,
    height: `${heightPx}px`,
    left: `calc(${leftPct}% + 1px)`,
    width: `calc(${widthPct}% - 2px)`,
    borderLeftColor: color,
    background: `color-mix(in oklab, ${color} 18%, var(--surface))`,
  };
  const withTime = showTime && heightPx >= TIME_MIN_H;
  const withLoc = showLocation && !!location && heightPx >= LOC_MIN_H;

  // A full accessible name (summary + time + place) even when the card is too short to show the meta.
  const ariaLabel = [summary, timeLabel, location].filter(Boolean).join(", ");

  return (
    <div
      className={cn(
        "absolute overflow-hidden rounded-[var(--radius-sm)] border border-border border-l-[3px] px-1.5 py-0.5",
        isPast && PAST_EVENT_CLASS,
      )}
      style={style}
      title={summary}
      aria-label={ariaLabel}
    >
      <div className="truncate font-head text-[11px] font-medium leading-tight text-ink">
        {summary}
      </div>
      {withTime && <div className="truncate font-mono text-[9px] text-ink4">{timeLabel}</div>}
      {withLoc && <div className="truncate font-mono text-[9px] text-ink4">{location}</div>}
    </div>
  );
}
