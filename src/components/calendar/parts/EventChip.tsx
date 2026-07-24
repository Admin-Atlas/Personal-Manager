// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A single-day event chip in the Month grid: a source-tinted pill with the title (and, in Power
// depth, its start time). The fill and left rule are the per-source colour mixed into transparency
// via color-mix, so the categorical hue arrives as a prop and no source hex is written in a component.
// Multi-day events render as bands (AllDayBand-style), not chips; Min depth collapses chips to dots.

import { cn } from "../../ui";
import { PAST_EVENT_CLASS } from "../../../lib/calendar-layout";

interface Props {
  summary: string;
  color: string;
  /** Local clock label for a timed event, e.g. "09:30"; empty for all-day. */
  timeLabel: string;
  /** Depth gate: show the time prefix (Power). */
  showTime: boolean;
  /** The event has fully passed — grey it back so what's done recedes. */
  isPast?: boolean;
  /** When set, the chip is interactive — click / Enter / Space opens the event's detail popup,
   *  anchored at the chip's on-screen rect. */
  onClick?: (anchor: DOMRect) => void;
}

export function EventChip({ summary, color, timeLabel, showTime, isPast, onClick }: Props) {
  return (
    <div
      className={cn(
        "flex items-center gap-1 overflow-hidden rounded-[var(--radius-sm)] border-l-[2px] px-1 py-px text-[11px] leading-tight",
        onClick && "cursor-pointer hover:brightness-110",
        isPast && PAST_EVENT_CLASS,
      )}
      style={{
        background: `color-mix(in oklab, ${color} 16%, transparent)`,
        borderLeftColor: color,
      }}
      title={summary}
      aria-label={[timeLabel, summary].filter(Boolean).join(", ")}
      role={onClick ? "button" : undefined}
      tabIndex={onClick ? 0 : undefined}
      onClick={onClick ? (e) => onClick(e.currentTarget.getBoundingClientRect()) : undefined}
      onKeyDown={
        onClick
          ? (e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                onClick(e.currentTarget.getBoundingClientRect());
              }
            }
          : undefined
      }
    >
      {showTime && timeLabel && (
        <span className="shrink-0 font-mono text-[9px] text-ink4">{timeLabel}</span>
      )}
      <span className="truncate font-head text-ink">{summary}</span>
    </div>
  );
}
