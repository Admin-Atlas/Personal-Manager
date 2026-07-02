// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The unified, read-only multi-calendar aggregator tab (card 8). Reads the widened mirror
// (listAllCalendarEvents) plus the account/calendar registry (calendarOverview), themes entirely
// from the global tokens, and colours each source from the categorical source palette. This PR adds
// the grid bodies (Day / Week time-grid, Month, Year) alongside the Agenda from PR1; the shared chrome
// drives per-view navigation. Terminal's mono treatments land in the next PR. Nothing here writes back.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { calendarOverview, listAllCalendarEvents, syncCalendar } from "../../lib/ipc";
import type { CalendarEvent, CalendarOverview } from "../../lib/types";
import {
  readHidden,
  readRange,
  readView,
  writeHidden,
  writeRange,
  writeView,
  type CalendarRange,
  type CalendarViewMode,
} from "../../lib/calendarPrefs";
import { formatDateLocal } from "../../lib/format";
import { addDays, startOfDay } from "../../lib/calendar-layout";
import { sourceColors, useTheme } from "../../theme";
import { Skeleton } from "../ui";
import { CalendarHeader } from "./CalendarHeader";
import { AgendaView } from "./views/AgendaView";
import { TimeGridView } from "./views/TimeGridView";
import { MonthView } from "./views/MonthView";
import { YearView } from "./views/YearView";

// The view switcher's order + the default for a fresh install. readView() clamps a persisted value to
// this set, so a value from a newer/older build never lands on a missing view.
const AVAILABLE_VIEWS: readonly CalendarViewMode[] = ["day", "week", "month", "year", "agenda"];
const DEFAULT_VIEW: CalendarViewMode = "month";

// Last-good data, kept in module scope so returning to the tab doesn't flash an empty grid before the
// mirror re-reads (mirrors FocusView's cache).
let cachedEvents: CalendarEvent[] = [];
let cachedOverview: CalendarOverview | null = null;

/** Monday-first start of the week containing `d`. */
function startOfWeek(d: Date): Date {
  const dow = (d.getDay() + 6) % 7;
  return addDays(startOfDay(d), -dow);
}

/** Step the cursor by one period in the current view's units. */
function stepCursor(view: CalendarViewMode, cur: Date, dir: number): Date {
  switch (view) {
    case "day":
      return addDays(cur, dir);
    case "week":
      return addDays(cur, 7 * dir);
    case "year":
      return new Date(cur.getFullYear() + dir, cur.getMonth(), 1);
    default: // month, agenda
      return new Date(cur.getFullYear(), cur.getMonth() + dir, 1);
  }
}

/** The period label shown between the nav arrows for each view. */
function viewLabel(view: CalendarViewMode, cur: Date): string {
  switch (view) {
    case "day":
      return `${cur.toLocaleDateString(undefined, { weekday: "long" })} ${formatDateLocal(cur)}`;
    case "week": {
      const monday = startOfWeek(cur);
      return `${formatDateLocal(monday)} – ${formatDateLocal(addDays(monday, 6))}`;
    }
    case "year":
      return String(cur.getFullYear());
    default: // month, agenda
      return cur.toLocaleDateString(undefined, { month: "long", year: "numeric" });
  }
}

export function CalendarView() {
  const { system, accent } = useTheme();
  const [overview, setOverview] = useState<CalendarOverview | null>(() => cachedOverview);
  const [events, setEvents] = useState<CalendarEvent[]>(() => cachedEvents);
  const [hidden, setHidden] = useState<Set<string>>(() => readHidden());
  const [view, setView] = useState<CalendarViewMode>(() => readView(AVAILABLE_VIEWS, DEFAULT_VIEW));
  const [range, setRange] = useState<CalendarRange>(() => readRange());
  const [cursor, setCursor] = useState<Date>(() => new Date());
  const [loading, setLoading] = useState(cachedOverview === null);
  const [syncing, setSyncing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const aliveRef = useRef(true);

  useEffect(() => {
    aliveRef.current = true;
    return () => {
      aliveRef.current = false;
    };
  }, []);

  const loadEvents = useCallback(async () => {
    try {
      const evts = await listAllCalendarEvents();
      if (!aliveRef.current) return;
      setEvents(evts);
      cachedEvents = evts;
    } catch {
      // Keep the last-good events; a read failure is transient.
    }
  }, []);

  const loadOverview = useCallback(async () => {
    try {
      const ov = await calendarOverview();
      if (!aliveRef.current) return;
      setOverview(ov);
      cachedOverview = ov;
    } catch {
      // Keep the last-good overview.
    } finally {
      if (aliveRef.current) setLoading(false);
    }
  }, []);

  // Initial load, and re-read the mirror when the window regains focus (the app-level poll or another
  // surface may have refreshed it while we were away).
  useEffect(() => {
    void loadOverview();
    void loadEvents();
    const onFocus = () => void loadEvents();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [loadOverview, loadEvents]);

  const onRefresh = useCallback(async () => {
    setSyncing(true);
    setError(null);
    try {
      await syncCalendar();
      await Promise.all([loadEvents(), loadOverview()]);
    } catch (e) {
      if (aliveRef.current) setError(e instanceof Error ? e.message : String(e));
    } finally {
      if (aliveRef.current) setSyncing(false);
    }
  }, [loadEvents, loadOverview]);

  const onToggleCalendar = useCallback((calendarId: string) => {
    setHidden((prev) => {
      const next = new Set(prev);
      if (next.has(calendarId)) next.delete(calendarId);
      else next.add(calendarId);
      writeHidden(next);
      return next;
    });
  }, []);

  const onViewChange = useCallback((v: CalendarViewMode) => {
    setView(v);
    writeView(v);
  }, []);

  const onRangeChange = useCallback((r: CalendarRange) => {
    setRange(r);
    writeRange(r);
  }, []);

  const onPrev = useCallback(() => setCursor((c) => stepCursor(view, c, -1)), [view]);
  const onNext = useCallback(() => setCursor((c) => stepCursor(view, c, 1)), [view]);
  const onToday = useCallback(() => setCursor(new Date()), []);
  const onPickDate = useCallback((d: Date) => setCursor(d), []);
  // Drilling from the Year view: jump to that day and open the Day view.
  const onSelectYearDay = useCallback(
    (d: Date) => {
      setCursor(d);
      onViewChange("day");
    },
    [onViewChange],
  );

  const colorOf = useMemo(() => {
    const ids = overview?.calendars.map((c) => c.id) ?? [];
    const map = sourceColors(ids, system, accent);
    return (calendarId: string) => map.get(calendarId) ?? "var(--ink4)";
  }, [overview, system, accent]);

  const visibleEvents = useMemo(
    () => events.filter((e) => !hidden.has(e.calendar_id)),
    [events, hidden],
  );

  const gridDays = useMemo<Date[]>(() => {
    if (view === "day") return [startOfDay(cursor)];
    if (view === "week") {
      const monday = startOfWeek(cursor);
      return Array.from({ length: 7 }, (_, i) => addDays(monday, i));
    }
    return [];
  }, [view, cursor]);

  const label = viewLabel(view, cursor);

  if (loading && !overview) {
    return (
      <div className="flex flex-1 flex-col gap-2 p-4">
        <Skeleton className="h-8 w-full" />
        <Skeleton className="h-full w-full" />
      </div>
    );
  }

  const hasCalendars = (overview?.calendars.length ?? 0) > 0;

  return (
    <div className="flex h-full min-h-0 flex-1 flex-col">
      <CalendarHeader
        view={view}
        availableViews={AVAILABLE_VIEWS}
        onViewChange={onViewChange}
        range={range}
        onRangeChange={onRangeChange}
        label={label}
        cursor={cursor}
        onPickDate={onPickDate}
        onPrev={onPrev}
        onNext={onNext}
        onToday={onToday}
        accounts={overview?.accounts ?? []}
        calendars={overview?.calendars ?? []}
        hidden={hidden}
        onToggleCalendar={onToggleCalendar}
        colorOf={colorOf}
        onRefresh={onRefresh}
        syncing={syncing}
        lastSync={overview?.last_sync ?? null}
      />

      {error && (
        <div
          className="border-b border-rule px-4 py-2 font-ui text-sm text-[var(--st-due)]"
          style={{ background: "color-mix(in oklab, var(--st-due) 15%, transparent)" }}
        >
          {error}
        </div>
      )}

      {!hasCalendars ? (
        <div className="flex flex-1 flex-col items-center justify-center gap-2 p-8 text-center">
          <p className="text-sm text-ink2">No calendars connected yet.</p>
          <p className="max-w-sm text-xs text-ink4">
            Connect a Google, Outlook, or iCal calendar in Settings → Connectors, then it appears
            here read-only.
          </p>
        </div>
      ) : view === "agenda" ? (
        <AgendaView events={visibleEvents} fromDay={cursor} colorOf={colorOf} />
      ) : view === "month" ? (
        <MonthView cursor={cursor} events={visibleEvents} colorOf={colorOf} />
      ) : view === "year" ? (
        <YearView cursor={cursor} events={visibleEvents} onSelectDay={onSelectYearDay} />
      ) : (
        <TimeGridView days={gridDays} events={visibleEvents} colorOf={colorOf} range={range} />
      )}
    </div>
  );
}
