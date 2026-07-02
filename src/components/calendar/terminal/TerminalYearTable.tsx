// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Terminal system's Year view: the same twelve mini-months as the shared YearView, but rendered in
// the mono/flat CLI treatment — JetBrains Mono labels and square day markers (MiniMonth shape="square").
// Event presence is derived from the real mirror exactly as in YearView; today stays the accent chip and
// the accent is reserved for it and the current-month label — sources never borrow the accent here.

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

interface Props {
  cursor: Date;
  /** Already filtered to visible (non-hidden) calendars. */
  events: CalendarEvent[];
  onSelectDay: (d: Date) => void;
}

const MONTHS = Array.from({ length: 12 }, (_, i) => i);

export function TerminalYearTable({ cursor, events, onSelectDay }: Props) {
  const year = cursor.getFullYear();
  const today = new Date();

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
    <div className="min-h-0 flex-1 overflow-y-auto p-4 font-mono">
      <div className="grid grid-cols-2 gap-x-6 gap-y-4 md:grid-cols-3 lg:grid-cols-4">
        {MONTHS.map((m) => (
          <div key={m}>
            <div
              className={cn(
                "mb-1 flex items-baseline gap-2 text-sm lowercase",
                m === currentMonth ? "text-accent-text" : "text-ink2",
              )}
            >
              {new Date(year, m, 1).toLocaleDateString(undefined, { month: "long" })}
              {m === currentMonth && <span className="text-[10px] text-accent-text">❮ now</span>}
            </div>
            <MiniMonth
              year={year}
              month={m}
              today={today}
              onSelectDay={onSelectDay}
              hasEvent={hasEvent}
              spans={spans}
              showWeekdays
              shape="square"
            />
          </div>
        ))}
      </div>
    </div>
  );
}
