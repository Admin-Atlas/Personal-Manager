// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Agenda view: synced events from the anchor day onward, grouped by local day. Each event reads
// its calendar's source colour as a left rule (the same move as the active nav item). This is also
// the layout the Terminal system will reuse for Week/Day (mono, no pixel grid) in a later PR.

import { useMemo } from "react";
import type { CalendarEvent } from "../../../lib/types";
import { formatClockIso, formatDateLocal } from "../../../lib/format";
import {
  dayKey,
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
  /** Show events on/after this day — including a multi-day event still running through it. */
  fromDay: Date;
  colorOf: (calendarId: string) => string;
  /** The ticking "now" (device-local) so an earlier-today event greys back. Defaults to the render clock. */
  now?: Date;
  /** Open an event's detail popup, anchored at the row's on-screen rect. */
  onEventClick?: (ev: CalendarEvent, anchor: DOMRect) => void;
}

/** An event's clock time for the agenda row: the local start time, or "all-day". */
function eventTime(ev: CalendarEvent): string {
  return ev.all_day ? "all-day" : formatClockIso(ev.start);
}

export function AgendaView({ events, fromDay, colorOf, now, onEventClick }: Props) {
  const { showMeta, showPower } = useDepth();
  const nowDate = now ?? new Date();

  const groups = useMemo(() => groupEventsFromDay(events, fromDay), [events, fromDay]);

  if (groups.length === 0) {
    return (
      <div className="flex flex-1 items-center justify-center p-8">
        <p className="text-sm text-ink4">No events in view.</p>
      </div>
    );
  }

  const todayKey = dayKey(startOfDay(nowDate));

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
                <div className="font-mono text-[0.625rem] text-accent-text">today</div>
              )}
            </div>
            <ul className="flex flex-1 flex-col gap-1">
              {g.items.map((ev) => {
                const clickable = !!onEventClick;
                return (
                  <li
                    key={ev.id}
                    className={cn(
                      "flex items-baseline gap-3 border-l-[3px] py-0.5 pl-2.5",
                      clickable && "cursor-pointer rounded-[var(--radius-sm)] hover:bg-surface",
                      isEventPast(ev, nowDate) && PAST_EVENT_CLASS,
                    )}
                    style={{ borderLeftColor: colorOf(ev.calendar_id) }}
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
                    <span className="w-14 shrink-0 font-mono text-xs text-ink4">
                      {eventTime(ev)}
                    </span>
                    <span className="truncate font-head text-sm text-ink">{ev.summary}</span>
                    {showPower && ev.location && (
                      <span className="truncate font-mono text-xs text-ink4">· {ev.location}</span>
                    )}
                  </li>
                );
              })}
            </ul>
          </section>
        );
      })}
    </div>
  );
}
