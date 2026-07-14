// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Year view: a continuously vertically-scrolling stream of mini-months laid out in width-
// responsive columns, with CSS scroll-snap to each MONTH row (never a whole year). Scrolling reports
// the year filling the view up via onFocusDate so the header tracks it; the nav arrows, Today and the
// mini-calendar scroll here. Event presence per day comes from the real mirror (soft disc / span pill),
// today is the accent circle, and the current real month's title reads in accent-text. Clicking a day
// drops the cursor there (the caller decides which view to open).

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
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
import { COL_GAP_PX, ROW_GAP_PX } from "../parts/useYearGridLayout";

interface Props {
  cursor: Date;
  /** Already filtered to visible (non-hidden) calendars. */
  events: CalendarEvent[];
  onSelectDay: (d: Date) => void;
  /** Reports the year filling the view as the user scrolls, so the header label can track it. */
  onFocusDate: (d: Date) => void;
}

const TITLE_PX = 26; // the month-name row above each mini-month
const WEEKDAY_PX = 16;
const MIN_CELL_PX = 18;
const MAX_CELL_PX = 40;
const PAST_MONTHS = 24; // window extent behind the anchor month…
const FUTURE_MONTHS = 24; // …and ahead — ~4yr of scroll each side

const monthKey = (d: Date) => `${d.getFullYear()}-${d.getMonth()}`;
const firstOfMonth = (d: Date) => new Date(d.getFullYear(), d.getMonth(), 1);

export function YearView({ cursor, events, onSelectDay, onFocusDate }: Props) {
  const today = new Date();
  const [anchor, setAnchor] = useState(() => firstOfMonth(cursor));
  const windowStart = useMemo(
    () => new Date(anchor.getFullYear(), anchor.getMonth() - PAST_MONTHS, 1),
    [anchor],
  );
  const months = useMemo(
    () =>
      Array.from(
        { length: PAST_MONTHS + FUTURE_MONTHS + 1 },
        (_, i) => new Date(windowStart.getFullYear(), windowStart.getMonth() + i, 1),
      ),
    [windowStart],
  );

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
  const hasEvent = useCallback((date: Date) => singleDays.has(dayKey(date)), [singleDays]);

  const scrollRef = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (el) setSize({ w: el.clientWidth, h: el.clientHeight });
  }, []);
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      const r = entries[0]?.contentRect;
      if (r) setSize({ w: r.width, h: r.height });
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const cols = size.w >= 1400 ? 4 : size.w >= 1000 ? 3 : size.w >= 620 ? 2 : 1;
  const colW = size.w > 0 ? (size.w - 32 - COL_GAP_PX * (cols - 1)) / cols : 0; // 32 = p-4 both sides
  const cellPx =
    colW > 0 ? Math.max(MIN_CELL_PX, Math.min(MAX_CELL_PX, Math.floor(colW / 7.4))) : 0;
  const monthH = TITLE_PX + WEEKDAY_PX + 6 * cellPx + 4;
  const rowStride = monthH + ROW_GAP_PX;
  const ready = cellPx > 0 && size.h > 0;

  const focusKeyRef = useRef(monthKey(cursor));
  const programmaticRef = useRef(false);
  const rafRef = useRef(0);
  const didInitRef = useRef(false);
  const prevAnchorRef = useRef(anchor);

  const monthOffset = useCallback(
    (d: Date) =>
      (d.getFullYear() - windowStart.getFullYear()) * 12 + (d.getMonth() - windowStart.getMonth()),
    [windowStart],
  );
  const inWindow = useCallback(
    (d: Date) => {
      const o = monthOffset(d);
      return o >= 0 && o < months.length;
    },
    [monthOffset, months.length],
  );
  const rowOfMonth = useCallback(
    (d: Date) => Math.floor(Math.max(0, Math.min(months.length - 1, monthOffset(d))) / cols),
    [monthOffset, months.length, cols],
  );

  const scrollToRow = useCallback(
    (row: number, smooth: boolean) => {
      const el = scrollRef.current;
      if (!el) return;
      programmaticRef.current = true;
      el.scrollTo({ top: row * rowStride, behavior: smooth ? "smooth" : "auto" });
      const clear = () => {
        programmaticRef.current = false;
        el.removeEventListener("scrollend", clear);
      };
      el.addEventListener("scrollend", clear);
      window.setTimeout(clear, 700);
    },
    [rowStride],
  );

  // Position on first paint (instant), window rebuild (instant), and external cursor move (smooth).
  // A scroll-driven cursor value has key === focusKeyRef and is skipped — no feedback loop.
  useLayoutEffect(() => {
    if (!ready) return;
    const key = monthKey(cursor);
    const anchorChanged = prevAnchorRef.current !== anchor;
    if (!anchorChanged && didInitRef.current && key === focusKeyRef.current) return;
    if (!inWindow(cursor)) {
      setAnchor(firstOfMonth(cursor));
      return;
    }
    const smooth = didInitRef.current && !anchorChanged;
    didInitRef.current = true;
    prevAnchorRef.current = anchor;
    focusKeyRef.current = key;
    scrollToRow(rowOfMonth(cursor), smooth);
  }, [cursor, anchor, ready, inWindow, rowOfMonth, scrollToRow]);

  const onScroll = useCallback(() => {
    if (programmaticRef.current || !ready) return;
    if (rafRef.current) return;
    rafRef.current = window.requestAnimationFrame(() => {
      rafRef.current = 0;
      const el = scrollRef.current;
      if (!el) return;
      const row = Math.max(0, Math.round((el.scrollTop + el.clientHeight * 0.4) / rowStride));
      const idx = Math.max(0, Math.min(months.length - 1, row * cols));
      const key = monthKey(months[idx]);
      if (key === focusKeyRef.current) return;
      focusKeyRef.current = key;
      onFocusDate(months[idx]);
    });
  }, [ready, rowStride, months, cols, onFocusDate]);

  useEffect(() => () => cancelAnimationFrame(rafRef.current), []);

  return (
    <div
      ref={scrollRef}
      onScroll={onScroll}
      className="min-h-0 flex-1 snap-y snap-proximity overflow-y-auto p-4"
    >
      {ready && (
        <div
          className="grid"
          style={{
            gridTemplateColumns: `repeat(${cols}, 1fr)`,
            columnGap: `${COL_GAP_PX}px`,
            rowGap: `${ROW_GAP_PX}px`,
          }}
        >
          {months.map((m) => {
            const isCurrent =
              m.getFullYear() === today.getFullYear() && m.getMonth() === today.getMonth();
            return (
              <div
                key={monthKey(m)}
                className="snap-start overflow-hidden"
                style={{ height: `${monthH}px` }}
              >
                <div
                  className={cn(
                    "mb-1 flex items-baseline gap-2 truncate font-head text-sm",
                    isCurrent ? "text-accent-text" : "text-ink",
                  )}
                  style={{ height: `${TITLE_PX}px` }}
                >
                  {m.toLocaleDateString(undefined, { month: "long" })}
                  <span className="font-mono text-[11px] text-ink4">{m.getFullYear()}</span>
                  {isCurrent && (
                    <span className="font-mono text-[10px] text-accent-text">this month</span>
                  )}
                </div>
                <MiniMonth
                  year={m.getFullYear()}
                  month={m.getMonth()}
                  today={today}
                  onSelectDay={onSelectDay}
                  hasEvent={hasEvent}
                  spans={spans}
                  showWeekdays
                  cellPx={cellPx}
                />
              </div>
            );
          })}
        </div>
      )}
    </div>
  );
}
