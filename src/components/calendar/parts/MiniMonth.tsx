// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A pure 7-column mini month, shared by the Year view and the header's mini-calendar picker. Renders
// day numbers with an optional weekday header, a filled accent circle for today, an accent ring for a
// selected day, a soft neutral disc for days that have events, and thin pills behind multi-day spans
// (packed per week row via calendar-layout). Optionally clickable. Neutral tints come from color-mix
// over tokens — no hex. Multi-day and single-day highlighting is derived by the caller from real data.

import { useMemo } from "react";
import {
  addDays,
  clampSpanToRange,
  dayDiff,
  dayKey,
  packBands,
  startOfDay,
  type BandInput,
} from "../../../lib/calendar-layout";
import { cn } from "../../ui";

export interface MiniSpan {
  startDay: Date;
  endDay: Date;
}

interface Props {
  year: number;
  month: number; // 0-11
  today: Date;
  selected?: Date | null;
  onSelectDay?: (d: Date) => void;
  /** Single-day event marker (soft disc behind the number). */
  hasEvent?: (date: Date) => boolean;
  /** Multi-day spans → thin pills behind the run. */
  spans?: MiniSpan[];
  showWeekdays?: boolean;
  /** Day-marker shape: round discs (Slate/Editorial) or square chips (Terminal). */
  shape?: "circle" | "square";
  /** Row height + day-badge size in px. Defaults to the compact size used by the header popover;
   *  the Year view passes a larger, container-fitted value so it isn't stuck at popover scale. */
  cellPx?: number;
}

const DEFAULT_CELL = 20;
const MAX_SPAN_LANES = 2;

export function MiniMonth({
  year,
  month,
  today,
  selected,
  onSelectDay,
  hasEvent,
  spans,
  showWeekdays,
  shape = "circle",
  cellPx = DEFAULT_CELL,
}: Props) {
  const CELL = cellPx;
  const badgePx = CELL;
  const fontPx = Math.max(10, Math.round(CELL * 0.42));
  const spanPx = Math.max(6, Math.round(CELL * 0.6));
  // Only the day-number markers swap to squares in Terminal; the multi-day pills stay rounded
  // (the handoff draws them with rounded ends on the first/last day across every System).
  const round = shape === "square" ? "rounded-[var(--radius-sm)]" : "rounded-full";
  const { weeks, weekdayLabels } = useMemo(() => {
    const first = new Date(year, month, 1);
    const lead = (first.getDay() + 6) % 7;
    const gridStart = addDays(startOfDay(first), -lead);
    const daysInMonth = new Date(year, month + 1, 0).getDate();
    const rows = Math.ceil((lead + daysInMonth) / 7);
    const weeks: Date[][] = [];
    for (let r = 0; r < rows; r++) {
      weeks.push(Array.from({ length: 7 }, (_, i) => addDays(gridStart, r * 7 + i)));
    }
    const weekdayLabels = Array.from({ length: 7 }, (_, i) =>
      addDays(gridStart, i).toLocaleDateString(undefined, { weekday: "narrow" }),
    );
    return { weeks, weekdayLabels };
  }, [year, month]);

  const todayKey = dayKey(startOfDay(today));
  const selectedKey = selected ? dayKey(startOfDay(selected)) : null;

  const spanBarsFor = (week: Date[]): { startIdx: number; endIdx: number; lane: number }[] => {
    if (!spans || spans.length === 0) return [];
    const weekStart = week[0];
    const inputs: BandInput[] = [];
    for (let i = 0; i < spans.length; i++) {
      const s = spans[i];
      const clamped = clampSpanToRange(
        dayDiff(weekStart, s.startDay),
        dayDiff(weekStart, s.endDay),
        6,
      );
      if (!clamped) continue;
      inputs.push({ id: String(i), startDay: clamped.startDay, endDay: clamped.endDay });
    }
    return packBands(inputs)
      .bands.filter((b) => b.lane < MAX_SPAN_LANES)
      .map((b) => ({ startIdx: b.startDay, endIdx: b.endDay, lane: b.lane }));
  };

  return (
    <div>
      {showWeekdays && (
        <div className="grid grid-cols-7">
          {weekdayLabels.map((w, i) => (
            <div
              key={i}
              className="text-center font-mono text-faint"
              style={{ fontSize: `${Math.max(9, Math.round(CELL * 0.35))}px` }}
            >
              {w}
            </div>
          ))}
        </div>
      )}
      {weeks.map((week, wi) => {
        const bars = spanBarsFor(week);
        return (
          <div key={wi} className="relative grid grid-cols-7" style={{ height: `${CELL}px` }}>
            {bars.map((b, bi) => (
              <div
                key={bi}
                aria-hidden
                className="pointer-events-none absolute top-1/2 -translate-y-1/2 rounded-full"
                style={{
                  left: `${(b.startIdx / 7) * 100}%`,
                  width: `${((b.endIdx - b.startIdx + 1) / 7) * 100}%`,
                  height: `${spanPx}px`,
                  background: "color-mix(in oklab, var(--ink) 14%, transparent)",
                }}
              />
            ))}
            {week.map((date) => {
              const key = dayKey(date);
              const isToday = key === todayKey;
              const isSelected = !isToday && key === selectedKey;
              const inMonth = date.getMonth() === month;
              const marked = !isToday && !isSelected && !!hasEvent?.(date);
              const numberEl = (
                <span
                  className={cn(
                    "relative flex items-center justify-center font-mono",
                    round,
                    isToday && "bg-accent font-medium text-accent-ink",
                    isSelected && "border border-accent text-accent-text",
                    !isToday && !isSelected && (inMonth ? "text-ink3" : "text-faint"),
                  )}
                  style={{
                    height: `${badgePx}px`,
                    width: `${badgePx}px`,
                    fontSize: `${fontPx}px`,
                    background: marked
                      ? "color-mix(in oklab, var(--ink) 14%, transparent)"
                      : undefined,
                  }}
                >
                  {date.getDate()}
                </span>
              );
              return (
                <div key={key} className="flex items-center justify-center">
                  {onSelectDay ? (
                    <button
                      type="button"
                      onClick={() => onSelectDay(date)}
                      className={cn("flex items-center justify-center hover:bg-surface", round)}
                      aria-label={date.toDateString()}
                    >
                      {numberEl}
                    </button>
                  ) : (
                    numberEl
                  )}
                </div>
              );
            })}
          </div>
        );
      })}
    </div>
  );
}
