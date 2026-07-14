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
import type { CalendarRange, RangeBounds } from "../../../lib/calendarPrefs";
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
import { ZoneGutter } from "../ZoneGutter";
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
  /** The visible-hour window this range frames (start/end in decimal hours). The grid still spans a
   *  scrollable full 24h — these only set the initial framing/scroll and the row scale. */
  bounds: RangeBounds;
  /** Up to 2 extra IANA zones to show as gutter columns beside the local time. */
  zones: string[];
  /** Add/remove an extra gutter zone — the corner control lives in the header row's gutter cell. */
  onZonesChange: (zones: string[]) => void;
  /** Open a milestone overlay event's project (fires for milestone all-day bands only). */
  onEventClick?: (ev: CalendarEvent) => void;
}

const LOCAL_COL = 54; // width of the local hour column (px)
const ZONE_COL = 46; // width of each extra-zone column (px)
const HOURS = 24;
const MIN_ROW_H = 20; // never scrunch a row below this
const MIN_WINDOW = 1; // guard divide-by-tiny in the row-height fill

/** The 24 hour labels for `zone`, formatting the same absolute instants the local column marks on
 *  `refDay` — DST-safe, and shows fractional offsets (Kolkata :30, Kathmandu :45) natively. */
function zoneHourLabels(refDay: Date, zone: string): string[] {
  try {
    const f = new Intl.DateTimeFormat("en-GB", {
      hour: "2-digit",
      minute: "2-digit",
      hourCycle: "h23",
      timeZone: zone,
    });
    return Array.from({ length: HOURS }, (_, h) =>
      f.format(new Date(refDay.getFullYear(), refDay.getMonth(), refDay.getDate(), h)),
    );
  } catch {
    return Array(HOURS).fill("");
  }
}

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

export function TimeGridView({
  days,
  events,
  colorOf,
  range,
  bounds,
  zones,
  onZonesChange,
  onEventClick,
}: Props) {
  const { minimal, showPower } = useDepth();
  // Derived from the framed window: scroll to its start on mount, stretch rows so the window fills
  // the body exactly. The grid itself always spans the full 24h; scrolling reaches the rest.
  const windowHours = Math.max(bounds.endHour - bounds.startHour, MIN_WINDOW);
  const scrollHour = bounds.startHour;
  const gutterPx = LOCAL_COL + zones.length * ZONE_COL;
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

  // Stretch rows so the framed window fills the body exactly. A narrow window (e.g. Work's ~9h) makes
  // tall rows and the full-24h grid scrolls; a wide one is floored at MIN_ROW_H so a short pane can't
  // crush rows below legibility (it scrolls instead). The grid always spans the full 24h — scrolling
  // reaches whatever sits outside the framed window.
  const rowH = bodyHeight > 0 ? Math.max(MIN_ROW_H, bodyHeight / windowHours) : MIN_ROW_H * 2;

  const bandEvents = useMemo(() => events.filter((e) => e.all_day || isMultiDay(e)), [events]);

  // The extra-zone gutter labels, computed against the first visible day (one shared axis across a
  // week — see the DST caveat below). Recomputed only when the zones or the anchor day change.
  const refDay = days[0];
  const zoneLabels = useMemo(
    () => zones.map((zone) => ({ zone, labels: refDay ? zoneHourLabels(refDay, zone) : [] })),
    [zones, refDay],
  );

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
        const startD = parseLocal(ev.start, false)!;
        const endD = ev.end ? parseLocal(ev.end, false) : null;
        return {
          ev,
          startMin: input.startMin,
          durMin: Math.max(input.endMin - input.startMin, 1),
          leftPct: (info.lane / info.lanes) * 100,
          widthPct: 100 / info.lanes,
          // Show start–end (en-dash); fall back to start-only when there's no/invalid end.
          timeLabel: endD ? `${formatClock(startD)}–${formatClock(endD)}` : formatClock(startD),
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
  // Include the start bound so editing a range's hours re-frames the scroll to the new start.
  const scrollKey = `${range}:${bounds.startHour}:${firstDayMs}`;
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
        <ZoneGutter
          zones={zones}
          onChange={onZonesChange}
          zoneCol={ZONE_COL}
          localCol={LOCAL_COL}
        />
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
        gutterPx={gutterPx}
        showLabel={!minimal}
        onEventClick={onEventClick}
      />

      {/* Scrollable time body */}
      <div ref={scrollRef} className="min-h-0 flex-1 overflow-y-auto">
        <div className="flex" style={{ height: `${HOURS * rowH}px` }}>
          {/* Hour gutter — extra zones then the local column (nearest the grid). */}
          <div className="flex shrink-0" style={{ width: `${gutterPx}px` }}>
            {zoneLabels.map(({ zone, labels }) => (
              <div
                key={zone}
                className="relative border-l border-rule"
                style={{ width: `${ZONE_COL}px` }}
              >
                {labels.map((lab, h) =>
                  h === 0 ? null : (
                    <div
                      key={h}
                      className="absolute right-1 -translate-y-1/2 font-mono text-[9px] text-ink4"
                      style={{ top: `${h * rowH}px` }}
                    >
                      {lab}
                    </div>
                  ),
                )}
              </div>
            ))}
            <div className="relative" style={{ width: `${LOCAL_COL}px` }}>
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
