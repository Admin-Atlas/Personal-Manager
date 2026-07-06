// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// One pixel time-grid engine for both Day (1 column) and Week (7). Hour rules are a
// repeating-linear-gradient in var(--rule); timed events are absolutely positioned from
// minutes-since-local-midnight (DST-tolerant, never an absolute UTC delta) and de-overlapped into
// equal-width lane columns via calendar-layout. All-day / multi-day events lift into the AllDayBand;
// timed events crossing midnight are already multi-day, so they lift too. Today's column gets an
// accent-soft tint and the now-line. Every colour is a token or the passed source colour — no hex.

import { useEffect, useMemo, useRef, useState } from "react";
import type { CalendarEvent } from "../../../lib/types";
import type { CalendarRange } from "../../../lib/calendarPrefs";
import {
  assignColumns,
  dayKey,
  eventDaySpan,
  isMultiDay,
  minutesFromLocalMidnight,
  parseLocal,
  timedEndMinutes,
  startOfDay,
  type TimedInput,
} from "../../../lib/calendar-layout";
import { formatClock } from "../../../lib/format";
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

// Vertical scale per range, plus the hour window each is framed on. The grid always spans the full
// 24h (nothing is ever un-scrollable-to) — `scrollHour` positions the body on mount, and
// `windowHours` is how many of those hours should fill the body exactly (stretching rowH beyond the
// preset when the body's taller than a fixed-size grid would need, so there's no dead space below).
const GEOM: Record<CalendarRange, { rowH: number; scrollHour: number; windowHours: number }> = {
  work: { rowH: 52, scrollHour: 8, windowHours: 24 },
  day: { rowH: 36, scrollHour: 8, windowHours: 12 },
  full: { rowH: 20, scrollHour: 0, windowHours: 24 },
};

// Position/size are kept as minute values so lane packing is independent of rowH — the pixel
// multiply happens at render, so a resize (which only changes rowH) never re-runs the packing memo.
interface CardGeom {
  ev: CalendarEvent;
  startMin: number;
  durMin: number;
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
  const { rowH: presetRowH, scrollHour, windowHours } = GEOM[range];
  const scrollRef = useRef<HTMLDivElement>(null);
  const [bodyHeight, setBodyHeight] = useState(0);

  // The body's visible height, independent of content — the flex layout sizes it, not the grid.
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      const h = entries[0]?.contentRect.height;
      if (h != null) setBodyHeight(h);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Stretch rows so the range's window (not necessarily the full 24h) fills the body exactly —
  // never shrink below the preset, so `work`'s tall business-hours grid still needs a scroll as
  // designed. The grid itself always spans the full 24h; scrolling reaches whatever's outside the
  // window (e.g. `day`'s 08–20 default framing).
  const rowH = bodyHeight > 0 ? Math.max(presetRowH, bodyHeight / windowHours) : presetRowH;

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
        const endMin = timedEndMinutes(start, end);
        return { id: ev.id, startMin, endMin };
      });
      const placed = assignColumns(inputs);
      const laneOf = new Map(placed.map((p) => [p.id, p]));
      // `inputs[i]` is built from `dayEvents[i]` in the same order, so index straight in — no O(n²)
      // id scan to recover the row we're already on.
      const cards: CardGeom[] = dayEvents.map((ev, i) => {
        const input = inputs[i];
        const info = laneOf.get(ev.id) ?? { lane: 0, lanes: 1 };
        return {
          ev,
          startMin: input.startMin,
          durMin: Math.max(input.endMin - input.startMin, 1),
          leftPct: (info.lane / info.lanes) * 100,
          widthPct: 100 / info.lanes,
          timeLabel: formatClock(parseLocal(ev.start, false)!),
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
    // No rowH dependency: the memo is pure minute-space, so resizing the pane (which only changes
    // rowH) doesn't re-run lane packing — the pixel multiply happens at render.
  }, [days, events, bandEvents]);

  // Auto-scroll to the range's window start ONCE per range/week (after the body is measured), not on
  // every resize — a resize only changes rowH, and re-running this would yank a scrolled-away user
  // back to the window start.
  const firstDayMs = days[0]?.getTime() ?? 0;
  const scrollKey = `${range}:${firstDayMs}`;
  const scrolledKeyRef = useRef<string | null>(null);
  useEffect(() => {
    const el = scrollRef.current;
    if (!el || bodyHeight === 0) return;
    if (scrolledKeyRef.current === scrollKey) return;
    el.scrollTop = scrollHour * rowH;
    scrolledKeyRef.current = scrollKey;
  }, [scrollKey, scrollHour, rowH, bodyHeight]);

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
                  topPx={(card.startMin / 60) * rowH}
                  heightPx={Math.max((card.durMin / 60) * rowH - 3, 14)}
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
