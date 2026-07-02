// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// One timed event in the pixel time-grid (Week/Day). Absolutely positioned by the geometry the view
// computes from calendar-layout; the card itself only paints. Surface + border are tokens; the single
// per-source colour is the left rule (the same move as the agenda row), passed in — never a hex here.
// Meta reveals with depth AND available height (a 20px sliver has no room for a time line).

import type { CSSProperties } from "react";

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
}: Props) {
  const style: CSSProperties = {
    top: `${topPx}px`,
    height: `${heightPx}px`,
    left: `calc(${leftPct}% + 1px)`,
    width: `calc(${widthPct}% - 2px)`,
    borderLeftColor: color,
  };
  const withTime = showTime && heightPx >= TIME_MIN_H;
  const withLoc = showLocation && !!location && heightPx >= LOC_MIN_H;

  return (
    <div
      className="absolute overflow-hidden rounded-[6px] border border-border border-l-[3px] bg-surface px-1.5 py-0.5"
      style={style}
      title={summary}
    >
      <div className="truncate font-head text-[11px] font-medium leading-tight text-ink">
        {summary}
      </div>
      {withTime && <div className="truncate font-mono text-[9px] text-ink4">{timeLabel}</div>}
      {withLoc && <div className="truncate font-mono text-[9px] text-ink4">{location}</div>}
    </div>
  );
}
