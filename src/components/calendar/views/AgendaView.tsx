// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Agenda view: synced events from the anchor day onward, grouped by local day. Each event reads
// its calendar's source colour as a left rule (the same move as the active nav item). This is also
// the layout the Terminal system will reuse for Week/Day (mono, no pixel grid) in a later PR.

import { useMemo } from "react";
import type { CalendarEvent } from "../../../lib/types";
import { formatDateLocal } from "../../../lib/format";
import { dayKey, eventDaySpan, startOfDay } from "../../../lib/calendar-layout";
import { useDepth } from "../../../theme";
import { cn } from "../../ui";

interface Props {
  /** Already filtered to visible (non-hidden) calendars. */
  events: CalendarEvent[];
  /** Show events whose local start day is on/after this day. */
  fromDay: Date;
  colorOf: (calendarId: string) => string;
}

interface DayGroup {
  day: Date;
  items: CalendarEvent[];
}

function weekdayShort(d: Date): string {
  return d.toLocaleDateString(undefined, { weekday: "short" });
}

/** An event's clock time for the agenda row: the local start time, or "all-day". */
function eventTime(ev: CalendarEvent): string {
  if (ev.all_day) return "all-day";
  const d = new Date(ev.start);
  if (Number.isNaN(d.getTime())) return "";
  return d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
}

export function AgendaView({ events, fromDay, colorOf }: Props) {
  const { showMeta, showPower } = useDepth();

  const groups = useMemo<DayGroup[]>(() => {
    const fromMs = startOfDay(fromDay).getTime();
    const byDay = new Map<string, DayGroup>();
    for (const ev of events) {
      const span = eventDaySpan(ev);
      if (!span || span.startDay.getTime() < fromMs) continue; // started before the anchor
      const key = dayKey(span.startDay);
      const g = byDay.get(key);
      if (g) g.items.push(ev);
      else byDay.set(key, { day: span.startDay, items: [ev] });
    }
    const ordered = [...byDay.values()].sort((a, b) => a.day.getTime() - b.day.getTime());
    for (const g of ordered) {
      // All-day first, then by start instant (ISO strings sort chronologically).
      g.items.sort((a, b) =>
        a.all_day !== b.all_day
          ? a.all_day
            ? -1
            : 1
          : String(a.start).localeCompare(String(b.start)),
      );
    }
    return ordered;
  }, [events, fromDay]);

  if (groups.length === 0) {
    return (
      <div className="flex flex-1 items-center justify-center p-8">
        <p className="text-sm text-ink4">No events in view.</p>
      </div>
    );
  }

  const todayKey = dayKey(startOfDay(new Date()));

  return (
    <div className="flex-1 overflow-y-auto px-4 py-2">
      {groups.map((g) => {
        const isToday = dayKey(g.day) === todayKey;
        return (
          <section
            key={dayKey(g.day)}
            className="flex gap-4 border-t border-rule py-3 first:border-t-0"
          >
            <div className="w-16 shrink-0">
              <div
                className={cn(
                  "font-head text-xs uppercase tracking-wide",
                  isToday ? "text-accent-text" : "text-ink3",
                )}
              >
                {weekdayShort(g.day)}
              </div>
              <div className={cn("font-mono text-sm", isToday ? "text-accent-text" : "text-ink2")}>
                {formatDateLocal(g.day)}
              </div>
              {isToday && showMeta && (
                <div className="font-mono text-[10px] text-accent-text">today</div>
              )}
            </div>
            <ul className="flex flex-1 flex-col gap-1">
              {g.items.map((ev) => (
                <li
                  key={ev.id}
                  className="flex items-baseline gap-3 border-l-[3px] py-0.5 pl-2.5"
                  style={{ borderLeftColor: colorOf(ev.calendar_id) }}
                >
                  <span className="w-14 shrink-0 font-mono text-xs text-ink4">{eventTime(ev)}</span>
                  <span className="truncate font-head text-sm text-ink">{ev.summary}</span>
                  {showPower && ev.location && (
                    <span className="truncate font-mono text-xs text-ink4">· {ev.location}</span>
                  )}
                </li>
              ))}
            </ul>
          </section>
        );
      })}
    </div>
  );
}
