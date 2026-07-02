// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Terminal system's stand-in for Week / Day / Agenda: a mono `cal` agenda, never a pixel grid.
// Each day is a `dow DD-MM` header rule (with a `❮ today` marker in accent-text on today) followed by
// `time ● title` rows, where `●` is the calendar's source colour. Bounded modes (Week/Day) list every
// day in the window and print a `·` for empty ones; the open-ended Agenda groups from the anchor day
// forward and omits empty days. Green (the accent) appears only on today — sources use the palette.

import { useMemo } from "react";
import type { CalendarEvent } from "../../../lib/types";
import { formatDateLocal } from "../../../lib/format";
import { dayKey, eventDaySpan, startOfDay } from "../../../lib/calendar-layout";
import { useDepth } from "../../../theme";
import { cn } from "../../ui";

interface Props {
  /** Already filtered to visible (non-hidden) calendars. */
  events: CalendarEvent[];
  colorOf: (calendarId: string) => string;
  /** Bounded window (Week = 7 local midnights, Day = 1). When set, every day is listed (empty → `·`). */
  days?: Date[];
  /** Open-ended Agenda anchor: group events on/after this day, omitting empty days. Ignored if `days`. */
  fromDay?: Date;
}

interface DayRow {
  day: Date;
  items: CalendarEvent[];
}

function weekdayShort(d: Date): string {
  return d.toLocaleDateString(undefined, { weekday: "short" });
}

/** All-day first, then by start instant (ISO strings sort chronologically). */
function sortItems(items: CalendarEvent[]): void {
  items.sort((a, b) =>
    a.all_day !== b.all_day ? (a.all_day ? -1 : 1) : String(a.start).localeCompare(String(b.start)),
  );
}

/** The row's time cell for `ev` on `day`: `all-day`, a continuation arrow for a multi-day event whose
 *  run started earlier, or the local `HH:MM` on its start day. */
function rowTime(ev: CalendarEvent, day: Date): string {
  if (ev.all_day) return "all-day";
  const start = new Date(ev.start);
  if (Number.isNaN(start.getTime())) return "";
  if (dayKey(startOfDay(start)) !== dayKey(day)) return "→";
  return start.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
}

export function TerminalAgenda({ events, colorOf, days, fromDay }: Props) {
  const { showPower } = useDepth();

  const rows = useMemo<DayRow[]>(() => {
    if (days && days.length > 0) {
      // Bounded: an event lands on every day its span covers, so a multi-day event shows on each day.
      return days.map((day) => {
        const dayMs = day.getTime();
        const items = events.filter((ev) => {
          const span = eventDaySpan(ev);
          return span && span.startDay.getTime() <= dayMs && dayMs <= span.endDay.getTime();
        });
        sortItems(items);
        return { day, items };
      });
    }
    // Open-ended agenda: group by local start day from the anchor forward, empty days omitted.
    const fromMs = startOfDay(fromDay ?? new Date()).getTime();
    const byDay = new Map<string, DayRow>();
    for (const ev of events) {
      const span = eventDaySpan(ev);
      if (!span || span.startDay.getTime() < fromMs) continue;
      const key = dayKey(span.startDay);
      const g = byDay.get(key);
      if (g) g.items.push(ev);
      else byDay.set(key, { day: span.startDay, items: [ev] });
    }
    const ordered = [...byDay.values()].sort((a, b) => a.day.getTime() - b.day.getTime());
    for (const g of ordered) sortItems(g.items);
    return ordered;
  }, [events, days, fromDay]);

  const hasAny = rows.some((r) => r.items.length > 0);
  if (!hasAny) {
    return (
      <div className="flex flex-1 items-center justify-center p-8 font-mono">
        <p className="text-sm text-ink4">No events in view.</p>
      </div>
    );
  }

  const todayKey = dayKey(startOfDay(new Date()));

  return (
    <div className="flex-1 overflow-y-auto px-4 py-2 font-mono">
      {rows.map((row) => {
        const isToday = dayKey(row.day) === todayKey;
        return (
          <section key={dayKey(row.day)} className="border-t border-rule py-2 first:border-t-0">
            <div className="mb-1 flex items-baseline gap-2 text-xs">
              <span className={cn(isToday ? "text-accent-text" : "text-ink2")}>
                {weekdayShort(row.day)} {formatDateLocal(row.day)}
              </span>
              {isToday && <span className="text-accent-text">❮ today</span>}
            </div>
            {row.items.length === 0 ? (
              <div className="pl-2 text-sm text-faint">·</div>
            ) : (
              <ul className="flex flex-col gap-0.5">
                {row.items.map((ev) => (
                  <li key={ev.id} className="flex items-baseline gap-2 text-sm">
                    <span className="w-16 shrink-0 text-ink4">{rowTime(ev, row.day)}</span>
                    <span aria-hidden style={{ color: colorOf(ev.calendar_id) }}>
                      ●
                    </span>
                    <span className="truncate text-ink">{ev.summary}</span>
                    {showPower && ev.location && (
                      <span className="truncate text-ink4">· {ev.location}</span>
                    )}
                  </li>
                ))}
              </ul>
            )}
          </section>
        );
      })}
    </div>
  );
}
