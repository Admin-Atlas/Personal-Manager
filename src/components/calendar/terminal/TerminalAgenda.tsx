// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Terminal system's stand-in for Week / Day / Agenda: a mono `cal` agenda, never a pixel grid.
// Each day is a `dow DD-MM` header rule (with a `❮ today` marker in accent-text on today) followed by
// `time ● title` rows, where `●` is the calendar's source colour. Bounded modes (Week/Day) list every
// day in the window and print a `·` for empty ones; the open-ended Agenda groups from the anchor day
// forward and omits empty days. Green (the accent) appears only on today — sources use the palette.

import { useMemo } from "react";
import type { CalendarEvent } from "../../../lib/types";
import { formatClock, formatDateLocal } from "../../../lib/format";
import {
  compareEventsForDay,
  dayKey,
  eventDaySpan,
  groupEventsFromDay,
  isEventPast,
  PAST_EVENT_CLASS,
  startOfDay,
  weekdayShort,
} from "../../../lib/calendar-layout";
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
  /** The ticking "now" (device-local) so a past event greys back. Defaults to the render clock. */
  now?: Date;
  /** Open an event's detail popup, anchored at the row's on-screen rect. */
  onEventClick?: (ev: CalendarEvent, anchor: DOMRect) => void;
}

interface DayRow {
  day: Date;
  items: CalendarEvent[];
}

/** The row's time cell for `ev` on `day`: `all-day`, a continuation arrow for a multi-day event whose
 *  run started earlier, or the local `HH:MM` on its start day. */
function rowTime(ev: CalendarEvent, day: Date): string {
  if (ev.all_day) return "all-day";
  const start = new Date(ev.start);
  if (Number.isNaN(start.getTime())) return "";
  if (dayKey(startOfDay(start)) !== dayKey(day)) return "→";
  return formatClock(start);
}

export function TerminalAgenda({ events, colorOf, days, fromDay, now, onEventClick }: Props) {
  const { showPower } = useDepth();
  const bounded = !!(days && days.length > 0);
  const nowDate = now ?? new Date();

  const rows = useMemo<DayRow[]>(() => {
    if (days && days.length > 0) {
      // Bounded: an event lands on every day its span covers, so a multi-day event shows on each day.
      return days.map((day) => {
        const dayMs = day.getTime();
        const items = events.filter((ev) => {
          const span = eventDaySpan(ev);
          return span && span.startDay.getTime() <= dayMs && dayMs <= span.endDay.getTime();
        });
        items.sort(compareEventsForDay);
        return { day, items };
      });
    }
    // Open-ended agenda: shared grouping (keeps an in-progress multi-day event on the anchor day).
    return groupEventsFromDay(events, fromDay ?? new Date());
  }, [events, days, fromDay]);

  // In a bounded (Week/Day) view every day is listed with a `·` skeleton for empty ones, so a
  // fully-empty week must still render its grid — only the open-ended agenda collapses to the panel.
  const hasAny = rows.some((r) => r.items.length > 0);
  if (!bounded && !hasAny) {
    return (
      <div className="flex flex-1 items-center justify-center p-8 font-mono">
        <p className="text-sm text-ink4">No events in view.</p>
      </div>
    );
  }

  const todayKey = dayKey(startOfDay(nowDate));

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
                {row.items.map((ev) => {
                  const clickable = !!onEventClick;
                  return (
                    <li
                      key={ev.id}
                      className={cn(
                        "flex items-baseline gap-2 text-sm",
                        clickable && "cursor-pointer rounded-[var(--radius-sm)] hover:bg-surface",
                        isEventPast(ev, nowDate) && PAST_EVENT_CLASS,
                      )}
                      role={clickable ? "button" : undefined}
                      tabIndex={clickable ? 0 : undefined}
                      onClick={
                        clickable
                          ? (e) => onEventClick?.(ev, e.currentTarget.getBoundingClientRect())
                          : undefined
                      }
                      onKeyDown={
                        clickable
                          ? (e) => {
                              if (e.key === "Enter" || e.key === " ") {
                                e.preventDefault();
                                onEventClick?.(ev, e.currentTarget.getBoundingClientRect());
                              }
                            }
                          : undefined
                      }
                    >
                      <span className="w-16 shrink-0 text-ink4">{rowTime(ev, row.day)}</span>
                      <span aria-hidden style={{ color: colorOf(ev.calendar_id) }}>
                        ●
                      </span>
                      {/* Free-height row: wrap rather than clip. Same reasoning as AgendaView —
                          and in Terminal this component also serves Day and Week. */}
                      <div className="min-w-0 flex-1">
                        <span className="break-words text-ink">{ev.summary}</span>
                        {showPower && ev.location && (
                          <div className="break-words text-ink4">{ev.location}</div>
                        )}
                      </div>
                    </li>
                  );
                })}
              </ul>
            )}
          </section>
        );
      })}
    </div>
  );
}
