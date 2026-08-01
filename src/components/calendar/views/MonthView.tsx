// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Month view: a continuously vertically-scrolling stream of Monday-first week rows spanning a
// generous window of months, with CSS scroll-snap to each WEEK (never a whole month). Days stay full-
// emphasis; the month name shows inline at each month's first day and alternating months carry a faint
// tint so boundaries read at a glance. Scrolling reports the month filling the view up via onFocusDate
// so the header label tracks it; the nav arrows, Today and the mini-calendar scroll here in turn. The
// per-week chips + multi-day band overlay reuse buildWeekRows, so they match the single-month grid.

import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { CalendarEvent } from "../../../lib/types";
import {
  addDays,
  dayKey,
  isEventPast,
  PAST_EVENT_CLASS,
  startOfDay,
} from "../../../lib/calendar-layout";
import { formatClockIso } from "../../../lib/format";
import { scrollBehavior, useDepth } from "../../../theme";
import { cn } from "../../ui";
import { EventChip } from "../parts/EventChip";
import { SourceDot } from "../parts/SourceDot";
import {
  buildWeekRows,
  weekStartMonday,
  weekdayLabels,
  type MonthWeekRow,
} from "../parts/useMonthGrid";

interface Props {
  /** The anchor day; its month is what the view scrolls to when it changes from outside. */
  cursor: Date;
  /** Already filtered to visible (non-hidden) calendars. */
  events: CalendarEvent[];
  colorOf: (calendarId: string) => string;
  /** The source's shape slot, for the colour-blind axis's redundant dot shapes. Optional — the dots
   *  fall back to plain circles without it (and whenever the axis is off). */
  shapeOf?: (calendarId: string) => number | undefined;
  /** Reports the month filling the view as the user scrolls, so the header label can track it. */
  onFocusDate: (d: Date) => void;
  /** The ticking "now" (device-local) so past chips/bands grey back. Defaults to the render clock. */
  now?: Date;
  /** Open an event's detail popup, anchored at the chip/band's on-screen rect. */
  onEventClick?: (ev: CalendarEvent, anchor: DOMRect) => void;
}

const BAND_H = 16;
const NUM_H = 22;
const CELL_PAD = 4; // matches the cells' py-1, so the row-relative band overlay clears the number row
const MAX_DOTS = 5;

const PAST_MONTHS = 20; // window extent behind the anchor month…
const FUTURE_MONTHS = 20; // …and ahead — ~3.3yr each side, well past the −1..+13mo event mirror
const VISIBLE_WEEKS = 6; // weeks that fill the pane (a full month) → sets the week-row height
const MIN_WEEK_H = 56;
const MS_PER_WEEK = 7 * 86_400_000;

const monthKey = (d: Date) => `${d.getFullYear()}-${d.getMonth()}`;
const firstOfMonth = (d: Date) => new Date(d.getFullYear(), d.getMonth(), 1);
/** A week's representative month = its Thursday (ISO), so a boundary week picks the majority side. */
const weekMonthKey = (w: MonthWeekRow) => monthKey(w.cells[3].date);

export function MonthView({
  cursor,
  events,
  colorOf,
  shapeOf,
  onFocusDate,
  now,
  onEventClick,
}: Props) {
  const { minimal, showMeta, showPower } = useDepth();
  const maxChips = showPower ? 4 : 3;
  const nowDate = now ?? new Date();

  // The rendered window is anchored on a month and rebuilt only when the cursor jumps outside it.
  const [anchor, setAnchor] = useState(() => firstOfMonth(cursor));
  const { weeks, gridStart } = useMemo(() => {
    const start = weekStartMonday(
      new Date(anchor.getFullYear(), anchor.getMonth() - PAST_MONTHS, 1),
    );
    const endExclusive = startOfDay(
      new Date(anchor.getFullYear(), anchor.getMonth() + FUTURE_MONTHS + 1, 1),
    );
    const rows = Math.round((endExclusive.getTime() - start.getTime()) / MS_PER_WEEK);
    return { weeks: buildWeekRows(start, rows, events), gridStart: start };
  }, [anchor, events]);
  const labels = useMemo(() => weekdayLabels(), []);

  const scrollRef = useRef<HTMLDivElement>(null);
  const [weekH, setWeekH] = useState(0);
  // The month the scroll last reported / was last driven to — the guard that breaks the feedback loop
  // between "scroll updates the cursor" and "a cursor change scrolls the view".
  const focusKeyRef = useRef(monthKey(cursor));
  const programmaticRef = useRef(false);
  const rafRef = useRef(0);
  const didInitRef = useRef(false);
  const prevAnchorRef = useRef(anchor);

  const weekIndexOfMonth = useCallback(
    (d: Date) => {
      const target = weekStartMonday(firstOfMonth(d));
      const idx = Math.round((target.getTime() - gridStart.getTime()) / MS_PER_WEEK);
      return Math.max(0, Math.min(weeks.length - 1, idx));
    },
    [gridStart, weeks.length],
  );

  const inWindow = useCallback(
    (d: Date) => {
      const f = firstOfMonth(d).getTime();
      const last = addDays(gridStart, weeks.length * 7 - 1).getTime();
      return f >= gridStart.getTime() && f <= last;
    },
    [gridStart, weeks.length],
  );

  const scrollToIndex = useCallback(
    (idx: number, smooth: boolean) => {
      const el = scrollRef.current;
      if (!el || weekH === 0) return;
      programmaticRef.current = true;
      el.scrollTo({ top: idx * weekH, behavior: scrollBehavior(smooth) });
      const clear = () => {
        programmaticRef.current = false;
        el.removeEventListener("scrollend", clear);
      };
      el.addEventListener("scrollend", clear);
      window.setTimeout(clear, 700); // safety: scrollend may not fire if already at the target
    },
    [weekH],
  );

  // Measure the pane → week-row height (a full month fills it), first synchronously to avoid a flash,
  // then track resizes.
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (el && el.clientHeight > 0) {
      setWeekH(Math.max(MIN_WEEK_H, Math.round(el.clientHeight / VISIBLE_WEEKS)));
    }
  }, []);
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      const h = entries[0]?.contentRect.height;
      if (h && h > 0) setWeekH(Math.max(MIN_WEEK_H, Math.round(h / VISIBLE_WEEKS)));
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // Position the view: first paint (instant), a window rebuild (instant), and an external cursor move
  // (smooth). A scroll-driven cursor value has key === focusKeyRef and is skipped — no feedback loop.
  useLayoutEffect(() => {
    if (weekH === 0) return;
    const key = monthKey(cursor);
    const anchorChanged = prevAnchorRef.current !== anchor;
    if (!anchorChanged && didInitRef.current && key === focusKeyRef.current) return;
    if (!inWindow(cursor)) {
      setAnchor(firstOfMonth(cursor)); // rebuild around the jumped-to month; repositions next pass
      return;
    }
    const smooth = didInitRef.current && !anchorChanged;
    didInitRef.current = true;
    prevAnchorRef.current = anchor;
    focusKeyRef.current = key;
    scrollToIndex(weekIndexOfMonth(cursor), smooth);
  }, [cursor, anchor, weekH, inWindow, weekIndexOfMonth, scrollToIndex]);

  const onScroll = useCallback(() => {
    if (programmaticRef.current || weekH === 0) return;
    if (rafRef.current) return;
    rafRef.current = window.requestAnimationFrame(() => {
      rafRef.current = 0;
      const el = scrollRef.current;
      if (!el) return;
      // The week ~40% down the pane is the one "in focus".
      const idx = Math.max(
        0,
        Math.min(weeks.length - 1, Math.round((el.scrollTop + el.clientHeight * 0.4) / weekH)),
      );
      const key = weekMonthKey(weeks[idx]);
      if (key === focusKeyRef.current) return;
      focusKeyRef.current = key;
      const [y, m] = key.split("-").map(Number);
      onFocusDate(new Date(y, m, 1));
    });
  }, [weekH, weeks, onFocusDate]);

  useEffect(() => () => cancelAnimationFrame(rafRef.current), []);

  return (
    <div className="flex h-full min-h-0 flex-1 flex-col">
      {/* Weekday header (fixed above the scroll) */}
      <div className="grid grid-cols-7 border-b border-rule">
        {labels.map((w, i) => (
          <div
            key={i}
            className="px-2 py-1 text-center font-head text-[0.6875rem] uppercase tracking-wide text-ink3"
          >
            {w}
          </div>
        ))}
      </div>

      {/* Scrolling week stream */}
      <div
        ref={scrollRef}
        onScroll={onScroll}
        className="min-h-0 flex-1 snap-y snap-proximity overflow-y-auto"
      >
        {weekH > 0 &&
          weeks.map((week) => (
            <div
              key={dayKey(week.cells[0].date)}
              className="relative grid snap-start grid-cols-7 border-b border-rule"
              style={{ height: `${weekH}px` }}
            >
              {week.cells.map((cell) => {
                const hidden = Math.max(0, cell.chips.length - maxChips) + cell.hiddenBands;
                const dotsHidden = Math.max(0, cell.chips.length - MAX_DOTS) + cell.hiddenBands;
                const firstOfM = cell.date.getDate() === 1;
                const tint = (cell.date.getFullYear() * 12 + cell.date.getMonth()) % 2 === 1;
                return (
                  <div
                    key={dayKey(cell.date)}
                    className={cn(
                      "flex min-h-0 flex-col overflow-hidden border-l border-rule px-1 py-1 first:border-l-0",
                      tint && "bg-panel",
                    )}
                  >
                    <div
                      className="flex items-start justify-between"
                      style={{ height: `${NUM_H}px` }}
                    >
                      <span className="flex items-center gap-1 overflow-hidden">
                        {firstOfM && (
                          <span className="truncate font-head text-[0.625rem] uppercase tracking-wide text-accent-text">
                            {cell.date.toLocaleDateString(undefined, { month: "short" })}
                            {cell.date.getMonth() === 0 ? ` ${cell.date.getFullYear()}` : ""}
                          </span>
                        )}
                        <span
                          className={cn(
                            "flex h-5 w-5 shrink-0 items-center justify-center font-mono text-xs",
                            cell.isToday
                              ? "rounded-full bg-accent font-medium text-accent-ink"
                              : "text-ink2",
                          )}
                        >
                          {cell.date.getDate()}
                        </span>
                      </span>
                      {showPower && cell.chips.length > 0 && (
                        <span className="font-mono text-[0.625rem] text-ink4">
                          {cell.chips.length}
                        </span>
                      )}
                    </div>

                    {/* Reserve room so chips clear the band overlay. */}
                    <div style={{ height: `${week.laneCount * BAND_H}px` }} />

                    {minimal ? (
                      <div className="flex flex-wrap items-center gap-0.5">
                        {cell.chips.slice(0, MAX_DOTS).map((ev) => (
                          <SourceDot
                            key={ev.id}
                            color={colorOf(ev.calendar_id)}
                            shapeIndex={shapeOf?.(ev.calendar_id)}
                            className="h-2 w-2"
                          />
                        ))}
                        {dotsHidden > 0 && (
                          <span className="font-mono text-[0.5625rem] text-ink4">
                            +{dotsHidden}
                          </span>
                        )}
                      </div>
                    ) : (
                      <div className="flex min-h-0 flex-col gap-0.5 overflow-hidden">
                        {cell.chips.slice(0, maxChips).map((ev) => (
                          <EventChip
                            key={ev.id}
                            summary={ev.summary}
                            color={colorOf(ev.calendar_id)}
                            timeLabel={ev.all_day ? "" : formatClockIso(ev.start)}
                            showTime={showMeta}
                            isPast={isEventPast(ev, nowDate)}
                            onClick={onEventClick ? (rect) => onEventClick(ev, rect) : undefined}
                          />
                        ))}
                        {hidden > 0 && (
                          <span className="pl-1 font-mono text-[0.625rem] text-ink4">
                            +{hidden} more
                          </span>
                        )}
                      </div>
                    )}
                  </div>
                );
              })}

              {/* Multi-day band overlay for this week row, aligned to the in-cell reserved spacer. */}
              <div
                className="pointer-events-none absolute inset-x-0"
                style={{ top: `${NUM_H + CELL_PAD}px` }}
              >
                {week.bands.map((b) => {
                  const color = colorOf(b.ev.calendar_id);
                  const leftPct = (b.startIdx / 7) * 100;
                  const widthPct = ((b.endIdx - b.startIdx + 1) / 7) * 100;
                  return (
                    <div
                      key={b.ev.id}
                      className={cn(
                        // The band overlay is pointer-events-none so day cells stay clickable; a band
                        // re-enables its own pointer events to open the event popup.
                        "absolute overflow-hidden px-1.5 text-[0.6875rem] leading-[0.875rem]",
                        onEventClick && "pointer-events-auto cursor-pointer hover:brightness-110",
                        isEventPast(b.ev, nowDate) && PAST_EVENT_CLASS,
                      )}
                      style={{
                        top: `${b.lane * BAND_H}px`,
                        left: `${leftPct}%`,
                        width: `${widthPct}%`,
                        height: `${BAND_H - 2}px`,
                        background: `color-mix(in oklab, ${color} 20%, transparent)`,
                        borderLeft: b.continuesLeft ? undefined : `3px solid ${color}`,
                        borderTopLeftRadius: b.continuesLeft ? 0 : "var(--radius-sm)",
                        borderBottomLeftRadius: b.continuesLeft ? 0 : "var(--radius-sm)",
                        borderTopRightRadius: b.continuesRight ? 0 : "var(--radius-sm)",
                        borderBottomRightRadius: b.continuesRight ? 0 : "var(--radius-sm)",
                      }}
                      title={b.ev.summary}
                      role={onEventClick ? "button" : undefined}
                      tabIndex={onEventClick ? 0 : undefined}
                      onClick={
                        onEventClick
                          ? (e) => onEventClick(b.ev, e.currentTarget.getBoundingClientRect())
                          : undefined
                      }
                      onKeyDown={
                        onEventClick
                          ? (e) => {
                              if (e.key === "Enter" || e.key === " ") {
                                e.preventDefault();
                                onEventClick(b.ev, e.currentTarget.getBoundingClientRect());
                              }
                            }
                          : undefined
                      }
                    >
                      <span className="truncate font-head text-ink">
                        {b.continuesLeft ? "‹ " : ""}
                        {b.ev.summary}
                      </span>
                    </div>
                  );
                })}
              </div>
            </div>
          ))}
      </div>
    </div>
  );
}
