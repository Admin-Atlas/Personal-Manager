// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Terminal system's Month grid: the same geometry as the shared MonthView (Monday-first weeks,
// calendar-layout.packBands multi-day overlay, single-day chips, depth-gated dots/counts) rendered in
// the mono/flat CLI treatment. Everything is JetBrains Mono, corners drop to --radius-sm squares, and
// chips lose their fill for a flat source-coloured left tick. Green (the accent) is reserved for
// today's number chip only — every source colour comes from the categorical palette, never the accent.

import type { CalendarEvent } from "../../../lib/types";
import { dayKey, isOverlayEvent } from "../../../lib/calendar-layout";
import { formatClockIso } from "../../../lib/format";
import { useDepth } from "../../../theme";
import { cn } from "../../ui";
import { useMonthGrid } from "../parts/useMonthGrid";

interface Props {
  /** A date within the month to render. */
  cursor: Date;
  /** Already filtered to visible (non-hidden) calendars. */
  events: CalendarEvent[];
  colorOf: (calendarId: string) => string;
  /** Open a PM overlay event — a milestone's project, or the Pinboard (fires for overlay chips only). */
  onEventClick?: (ev: CalendarEvent) => void;
}

const BAND_H = 16;
const NUM_H = 22;
const CELL_PAD = 4; // matches the cells' py-1, so the row-relative band overlay clears the number row
const MAX_DOTS = 5;

export function TerminalMonthTable({ cursor, events, colorOf, onEventClick }: Props) {
  const { minimal, showMeta, showPower } = useDepth();
  const maxChips = showPower ? 4 : 3;

  const { weeks, weekdayLabels } = useMonthGrid(cursor, events);

  return (
    <div className="flex h-full min-h-0 flex-1 flex-col font-mono">
      {/* Weekday header — mono, lowercased for the CLI feel. */}
      <div className="grid grid-cols-7 border-b border-rule">
        {weekdayLabels.map((w, i) => (
          <div
            key={i}
            className="px-2 py-1 text-center text-[10px] lowercase tracking-wide text-ink3"
          >
            {w}
          </div>
        ))}
      </div>

      {/* Week rows */}
      <div className="flex min-h-0 flex-1 flex-col">
        {weeks.map((week, wi) => (
          <div
            key={wi}
            className="relative grid min-h-0 flex-1 grid-cols-7 border-b border-rule last:border-b-0"
          >
            {week.cells.map((cell) => {
              const hiddenCount = Math.max(0, cell.chips.length - maxChips) + cell.hiddenBands;
              const dotsHidden = Math.max(0, cell.chips.length - MAX_DOTS) + cell.hiddenBands;
              return (
                <div
                  key={dayKey(cell.date)}
                  className={cn(
                    "flex min-h-0 flex-col overflow-hidden border-l border-rule px-1 py-1 first:border-l-0",
                    !cell.inMonth && "bg-panel",
                  )}
                >
                  <div
                    className="flex items-start justify-between"
                    style={{ height: `${NUM_H}px` }}
                  >
                    <span
                      className={cn(
                        "flex h-5 w-5 items-center justify-center text-xs",
                        cell.isToday &&
                          "rounded-[var(--radius-sm)] bg-accent font-medium text-accent-ink",
                        !cell.isToday && (cell.inMonth ? "text-ink2" : "text-faint"),
                      )}
                    >
                      {cell.date.getDate()}
                    </span>
                    {showPower && cell.chips.length > 0 && (
                      <span className="text-[10px] text-ink4">{cell.chips.length}</span>
                    )}
                  </div>

                  {/* Reserve room so chips clear the band overlay. */}
                  <div style={{ height: `${week.laneCount * BAND_H}px` }} />

                  {minimal ? (
                    <div className="flex flex-wrap items-center gap-0.5">
                      {cell.chips.slice(0, MAX_DOTS).map((ev) => (
                        <span
                          key={ev.id}
                          aria-hidden
                          className="inline-block h-2 w-2 shrink-0 rounded-[var(--radius-sm)]"
                          style={{ backgroundColor: colorOf(ev.calendar_id) }}
                        />
                      ))}
                      {dotsHidden > 0 && (
                        <span className="text-[9px] text-ink4">+{dotsHidden}</span>
                      )}
                    </div>
                  ) : (
                    <div className="flex min-h-0 flex-col gap-0.5 overflow-hidden">
                      {cell.chips.slice(0, maxChips).map((ev) => {
                        const clickable = isOverlayEvent(ev);
                        return (
                          <div
                            key={ev.id}
                            className={cn(
                              "flex items-center gap-1 overflow-hidden border-l-2 pl-1 text-[11px] leading-tight",
                              clickable && "cursor-pointer hover:brightness-110",
                            )}
                            style={{ borderLeftColor: colorOf(ev.calendar_id) }}
                            title={ev.summary}
                            role={clickable ? "button" : undefined}
                            tabIndex={clickable ? 0 : undefined}
                            onClick={clickable ? () => onEventClick?.(ev) : undefined}
                            onKeyDown={
                              clickable
                                ? (e) => {
                                    if (e.key === "Enter" || e.key === " ") {
                                      e.preventDefault();
                                      onEventClick?.(ev);
                                    }
                                  }
                                : undefined
                            }
                          >
                            {showMeta && !ev.all_day && formatClockIso(ev.start) && (
                              <span className="shrink-0 text-[9px] text-ink4">
                                {formatClockIso(ev.start)}
                              </span>
                            )}
                            <span className="truncate text-ink">{ev.summary}</span>
                          </div>
                        );
                      })}
                      {hiddenCount > 0 && (
                        <span className="pl-1 text-[10px] text-ink4">+{hiddenCount} more</span>
                      )}
                    </div>
                  )}
                </div>
              );
            })}

            {/* Multi-day band overlay for this week row, aligned to the in-cell reserved spacer. A flat
                source-coloured left tick (no fill) with the mono title — the width conveys the span. */}
            <div
              className="pointer-events-none absolute inset-x-0"
              style={{ top: `${NUM_H + CELL_PAD}px` }}
            >
              {week.bands.map((b) => {
                const color = colorOf(b.ev.calendar_id);
                const leftPct = (b.startIdx / 7) * 100;
                const widthPct = ((b.endIdx - b.startIdx + 1) / 7) * 100;
                return (
                  <div
                    key={b.ev.id}
                    className="absolute overflow-hidden px-1.5 text-[11px] leading-[14px]"
                    style={{
                      top: `${b.lane * BAND_H}px`,
                      left: `${leftPct}%`,
                      width: `${widthPct}%`,
                      height: `${BAND_H - 2}px`,
                      background: `color-mix(in oklab, ${color} 14%, transparent)`,
                      borderLeft: b.continuesLeft ? undefined : `3px solid ${color}`,
                      borderTopLeftRadius: b.continuesLeft ? 0 : "var(--radius-sm)",
                      borderBottomLeftRadius: b.continuesLeft ? 0 : "var(--radius-sm)",
                      borderTopRightRadius: b.continuesRight ? 0 : "var(--radius-sm)",
                      borderBottomRightRadius: b.continuesRight ? 0 : "var(--radius-sm)",
                    }}
                    title={b.ev.summary}
                  >
                    <span className="truncate text-ink">
                      {b.continuesLeft ? "‹ " : ""}
                      {b.ev.summary}
                    </span>
                  </div>
                );
              })}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
