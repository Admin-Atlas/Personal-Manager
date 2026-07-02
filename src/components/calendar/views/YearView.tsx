// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Year view: twelve mini-months for the cursor's year. Event presence is derived from the real
// mirror — single-day events mark a soft disc, multi-day events draw a pill behind their run, today is
// the accent circle. Clicking a day drops the cursor there (the caller decides which view to open).
// The current month's label reads in accent-text. All colour is tokens or color-mix over tokens.

import { useMemo } from "react";
import type { CalendarEvent } from "../../../lib/types";
import {
  dayKey,
  eventDaySpan,
  isMultiDay,
  parseLocal,
  startOfDay,
} from "../../../lib/calendar-layout";
import { cn } from "../../ui";
import { MiniMonth, type MiniSpan } from "../parts/MiniMonth";
import { COL_GAP_PX, ROW_GAP_PX, useYearGridLayout } from "../parts/useYearGridLayout";

interface Props {
  cursor: Date;
  /** Already filtered to visible (non-hidden) calendars. */
  events: CalendarEvent[];
  onSelectDay: (d: Date) => void;
}

const MONTHS = Array.from({ length: 12 }, (_, i) => i);

export function YearView({ cursor, events, onSelectDay }: Props) {
  const year = cursor.getFullYear();
  const today = new Date();
  const { containerRef, cols, rows, cellPx } = useYearGridLayout(MONTHS.length);

  const { singleDays, spans } = useMemo(() => {
    const singleDays = new Set<string>();
    const spans: MiniSpan[] = [];
    for (const ev of events) {
      if (isMultiDay(ev)) {
        const span = eventDaySpan(ev);
        if (span) spans.push({ startDay: span.startDay, endDay: span.endDay });
        continue;
      }
      const start = parseLocal(ev.start, ev.all_day);
      if (start) singleDays.add(dayKey(startOfDay(start)));
    }
    return { singleDays, spans };
  }, [events]);

  const hasEvent = (date: Date) => singleDays.has(dayKey(date));
  const currentMonth = today.getFullYear() === year ? today.getMonth() : -1;

  return (
    <div ref={containerRef} className="min-h-0 flex-1 overflow-y-auto p-4">
      <div
        className="grid"
        style={{
          height: "100%",
          gridTemplateColumns: `repeat(${cols}, 1fr)`,
          // minmax(0, 1fr) lets a row shrink below its content's natural size instead of growing the
          // grid past the container (a slightly-off cellPx estimate then clips a hair off one month's
          // last week row via the per-cell overflow-hidden below, rather than forcing an outer scrollbar).
          gridTemplateRows: `repeat(${rows}, minmax(0, 1fr))`,
          columnGap: `${COL_GAP_PX}px`,
          rowGap: `${ROW_GAP_PX}px`,
        }}
      >
        {MONTHS.map((m) => (
          <div key={m} className="flex h-full flex-col justify-center overflow-hidden">
            <div
              className={cn(
                "mb-1 flex items-baseline gap-2 font-head text-sm",
                m === currentMonth ? "text-accent-text" : "text-ink",
              )}
            >
              {new Date(year, m, 1).toLocaleDateString(undefined, { month: "long" })}
              {m === currentMonth && (
                <span className="font-mono text-[10px] text-accent-text">this month</span>
              )}
            </div>
            <MiniMonth
              year={year}
              month={m}
              today={today}
              onSelectDay={onSelectDay}
              hasEvent={hasEvent}
              spans={spans}
              showWeekdays
              cellPx={cellPx}
            />
          </div>
        ))}
      </div>
    </div>
  );
}
