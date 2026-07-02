// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Month grid: Monday-first, one row per week, cells flex to fill. Multi-day events pack into
// stacked bands drawn as a per-week overlay (calendar-layout.packBands + clampSpanToRange, so a span
// crossing the week edge gets a flat/continuation edge); single-day events render as source-tinted
// chips, collapsing to a row of source dots in Min depth. Today is a filled accent circle; adjacent-
// month days recede to panel/faint. Colours are tokens or the passed source colour — never a hex.

import type { CalendarEvent } from "../../../lib/types";
import { dayKey } from "../../../lib/calendar-layout";
import { formatClockIso } from "../../../lib/format";
import { useDepth } from "../../../theme";
import { cn } from "../../ui";
import { EventChip } from "../parts/EventChip";
import { SourceDot } from "../parts/SourceDot";
import { useMonthGrid } from "../parts/useMonthGrid";

interface Props {
  /** A date within the month to render. */
  cursor: Date;
  /** Already filtered to visible (non-hidden) calendars. */
  events: CalendarEvent[];
  colorOf: (calendarId: string) => string;
}

const BAND_H = 16;
const NUM_H = 22;
const CELL_PAD = 4; // matches the cells' py-1, so the row-relative band overlay clears the number row
const MAX_DOTS = 5;

export function MonthView({ cursor, events, colorOf }: Props) {
  const { minimal, showMeta, showPower } = useDepth();
  const maxChips = showPower ? 4 : 3;

  const { weeks, weekdayLabels } = useMonthGrid(cursor, events);

  return (
    <div className="flex h-full min-h-0 flex-1 flex-col">
      {/* Weekday header */}
      <div className="grid grid-cols-7 border-b border-rule">
        {weekdayLabels.map((w, i) => (
          <div
            key={i}
            className="px-2 py-1 text-center font-head text-[11px] uppercase tracking-wide text-ink3"
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
              // Overflow = chips past the cap PLUS any multi-day bands dropped past the lane cap, so
              // over-full days never silently swallow events (whichever kind).
              const hidden = Math.max(0, cell.chips.length - maxChips) + cell.hiddenBands;
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
                        "flex h-5 w-5 items-center justify-center font-mono text-xs",
                        cell.isToday && "rounded-full bg-accent font-medium text-accent-ink",
                        !cell.isToday && (cell.inMonth ? "text-ink2" : "text-faint"),
                      )}
                    >
                      {cell.date.getDate()}
                    </span>
                    {showPower && cell.chips.length > 0 && (
                      <span className="font-mono text-[10px] text-ink4">{cell.chips.length}</span>
                    )}
                  </div>

                  {/* Reserve room so chips clear the band overlay. */}
                  <div style={{ height: `${week.laneCount * BAND_H}px` }} />

                  {minimal ? (
                    <div className="flex flex-wrap items-center gap-0.5">
                      {cell.chips.slice(0, MAX_DOTS).map((ev) => (
                        <SourceDot
                          key={ev.id}
                          color={colorOf(ev.calendar_id)}
                          className="h-2 w-2"
                        />
                      ))}
                      {dotsHidden > 0 && (
                        <span className="font-mono text-[9px] text-ink4">+{dotsHidden}</span>
                      )}
                    </div>
                  ) : (
                    <div className="flex min-h-0 flex-col gap-0.5 overflow-hidden">
                      {cell.chips.slice(0, maxChips).map((ev) => (
                        <EventChip
                          key={ev.id}
                          summary={ev.summary}
                          color={colorOf(ev.calendar_id)}
                          timeLabel={ev.all_day ? "" : formatClockIso(ev.start)}
                          showTime={showMeta}
                        />
                      ))}
                      {hidden > 0 && (
                        <span className="pl-1 font-mono text-[10px] text-ink4">+{hidden} more</span>
                      )}
                    </div>
                  )}
                </div>
              );
            })}

            {/* Multi-day band overlay for this week row, aligned to the in-cell reserved spacer. */}
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
                      background: `color-mix(in oklab, ${color} 20%, transparent)`,
                      borderLeft: b.continuesLeft ? undefined : `3px solid ${color}`,
                      borderTopLeftRadius: b.continuesLeft ? 0 : "var(--radius-sm)",
                      borderBottomLeftRadius: b.continuesLeft ? 0 : "var(--radius-sm)",
                      borderTopRightRadius: b.continuesRight ? 0 : "var(--radius-sm)",
                      borderBottomRightRadius: b.continuesRight ? 0 : "var(--radius-sm)",
                    }}
                    title={b.ev.summary}
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
        ))}
      </div>
    </div>
  );
}
