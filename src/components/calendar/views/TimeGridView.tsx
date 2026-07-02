// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// One pixel time-grid engine for both Day (1 column) and Week (7). Hour rules are a
// repeating-linear-gradient in var(--rule); timed events are absolutely positioned from
// minutes-since-local-midnight (DST-tolerant, never an absolute UTC delta) and de-overlapped into
// equal-width lane columns via calendar-layout. All-day / multi-day events lift into the AllDayBand;
// timed events crossing midnight are already multi-day, so they lift too. Today's column gets an
// accent-soft tint and the now-line. Every colour is a token or the passed source colour — no hex.

import { useEffect, useMemo, useRef } from "react";
import type { CalendarEvent } from "../../../lib/types";
import type { CalendarRange } from "../../../lib/calendarPrefs";
import {
  assignColumns,
  dayKey,
  eventDaySpan,
  isMultiDay,
  minutesFromLocalMidnight,
  parseLocal,
  startOfDay,
  type TimedInput,
} from "../../../lib/calendar-layout";
import { useDepth } from "../../../theme";
import { cn } from "../../ui";
import { EventCard } from "../parts/EventCard";
import { NowLine } from "../parts/NowLine";
import { AllDayBand } from "../parts/AllDayBand";

interface Props {
  /** The visible day columns (local midnights), left → right. Day = 1, Week = 7. */
  days: Date[];
  /** Already filtered to visible (non-hidden) calendars. */
  events: CalendarEvent[];
  colorOf: (calendarId: string) => string;
  range: CalendarRange;
}

const GUTTER_PX = 54;
const HOURS = 24;

// Vertical scale + where the body scrolls to on mount, per range.
const GEOM: Record<CalendarRange, { rowH: number; scrollHour: number }> = {
  work: { rowH: 52, scrollHour: 8 },
  day: { rowH: 36, scrollHour: 6 },
  full: { rowH: 20, scrollHour: 0 },
};

function hhmm(d: Date): string {
  return d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
}

interface CardGeom {
  ev: CalendarEvent;
  topPx: number;
  heightPx: number;
  leftPct: number;
  widthPct: number;
  timeLabel: string;
}

interface DayColumn {
  day: Date;
  isToday: boolean;
  count: number;
  cards: CardGeom[];
}

export function TimeGridView({ days, events, colorOf, range }: Props) {
  const { minimal, showPower } = useDepth();
  const { rowH, scrollHour } = GEOM[range];
  const scrollRef = useRef<HTMLDivElement>(null);

  const bandEvents = useMemo(() => events.filter((e) => e.all_day || isMultiDay(e)), [events]);

  const columns = useMemo<DayColumn[]>(() => {
    const todayKey = dayKey(startOfDay(new Date()));
    // Bucket single-day timed events by their local start day.
    const timedByDay = new Map<string, CalendarEvent[]>();
    for (const ev of events) {
      if (ev.all_day || isMultiDay(ev)) continue;
      const start = parseLocal(ev.start, false);
      if (!start) continue;
      const key = dayKey(startOfDay(start));
      const list = timedByDay.get(key);
      if (list) list.push(ev);
      else timedByDay.set(key, [ev]);
    }
    return days.map((day) => {
      const key = dayKey(day);
      const dayEvents = timedByDay.get(key) ?? [];
      const inputs: TimedInput[] = dayEvents.map((ev) => {
        const start = parseLocal(ev.start, false)!;
        const end = ev.end ? parseLocal(ev.end, false) : null;
        const startMin = minutesFromLocalMidnight(start);
        const endMin = end ? minutesFromLocalMidnight(end) : startMin + 30;
        return { id: ev.id, startMin, endMin };
      });
      const placed = assignColumns(inputs);
      const laneOf = new Map(placed.map((p) => [p.id, p]));
      const cards: CardGeom[] = dayEvents.map((ev) => {
        const input = inputs.find((i) => i.id === ev.id)!;
        const info = laneOf.get(ev.id) ?? { lane: 0, lanes: 1 };
        const durMin = Math.max(input.endMin - input.startMin, 1);
        const start = parseLocal(ev.start, false)!;
        return {
          ev,
          topPx: (input.startMin / 60) * rowH,
          heightPx: Math.max((durMin / 60) * rowH - 3, 14),
          leftPct: (info.lane / info.lanes) * 100,
          widthPct: 100 / info.lanes,
          timeLabel: hhmm(start),
        };
      });
      // Total events touching the day (timed + bands overlapping), for the Power count line.
      let count = dayEvents.length;
      for (const ev of bandEvents) {
        const span = eventDaySpan(ev);
        if (
          span &&
          span.startDay.getTime() <= day.getTime() &&
          day.getTime() <= span.endDay.getTime()
        ) {
          count++;
        }
      }
      return { day, isToday: key === todayKey, count, cards };
    });
  }, [days, events, bandEvents, rowH]);

  // Auto-scroll the body to the range's window start on mount and whenever the range or visible week
  // changes — the spec's "scroll to the working window" behaviour.
  const firstDayMs = days[0]?.getTime() ?? 0;
  useEffect(() => {
    const el = scrollRef.current;
    if (el) el.scrollTop = scrollHour * rowH;
  }, [scrollHour, rowH, firstDayMs]);

  const nowMin = minutesFromLocalMidnight(new Date());
  const hourLines = `repeating-linear-gradient(to bottom, var(--rule) 0, var(--rule) 1px, transparent 1px, transparent ${rowH}px)`;

  return (
    <div className="flex h-full min-h-0 flex-1 flex-col">
      {/* Day-header row */}
      <div className="flex border-b border-rule">
        <div className="shrink-0" style={{ width: `${GUTTER_PX}px` }} />
        {columns.map((c) => (
          <div key={dayKey(c.day)} className="flex-1 border-l border-rule px-2 py-1 text-center">
            <div
              className={cn(
                "font-head text-xs uppercase tracking-wide",
                c.isToday ? "text-accent-text" : "text-ink3",
              )}
            >
              {c.day.toLocaleDateString(undefined, { weekday: "short" })}
            </div>
            <div className={cn("font-mono text-sm", c.isToday ? "text-accent-text" : "text-ink2")}>
              {c.day.getDate()}
            </div>
            {showPower && <div className="font-mono text-[10px] text-ink4">{c.count} events</div>}
          </div>
        ))}
      </div>

      <AllDayBand
        events={bandEvents}
        days={days}
        colorOf={colorOf}
        gutterPx={GUTTER_PX}
        showLabel={!minimal}
      />

      {/* Scrollable time body */}
      <div ref={scrollRef} className="min-h-0 flex-1 overflow-y-auto">
        <div className="flex" style={{ height: `${HOURS * rowH}px` }}>
          {/* Hour gutter */}
          <div className="relative shrink-0" style={{ width: `${GUTTER_PX}px` }}>
            {Array.from({ length: HOURS }, (_, h) => h).map((h) => (
              <div
                key={h}
                className="absolute right-2 -translate-y-1/2 font-mono text-[10px] text-ink4"
                style={{ top: `${h * rowH}px` }}
              >
                {h === 0 ? "" : `${String(h).padStart(2, "0")}:00`}
              </div>
            ))}
          </div>
          {/* Day columns */}
          {columns.map((c) => (
            <div
              key={dayKey(c.day)}
              className={cn("relative flex-1 border-l border-rule", c.isToday && "bg-accent-soft")}
              style={{ background: c.isToday ? undefined : hourLines }}
            >
              {c.isToday && (
                <div
                  className="pointer-events-none absolute inset-0"
                  style={{ background: hourLines }}
                />
              )}
              {c.cards.map((card) => (
                <EventCard
                  key={card.ev.id}
                  summary={card.ev.summary}
                  color={colorOf(card.ev.calendar_id)}
                  timeLabel={card.timeLabel}
                  location={card.ev.location}
                  topPx={card.topPx}
                  heightPx={card.heightPx}
                  leftPct={card.leftPct}
                  widthPct={card.widthPct}
                  showTime={!minimal}
                  showLocation={showPower}
                />
              ))}
              {c.isToday && <NowLine topPx={(nowMin / 60) * rowH} />}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
