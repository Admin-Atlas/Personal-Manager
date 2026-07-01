// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The unified, read-only multi-calendar aggregator tab (card 8). Reads the widened mirror
// (listAllCalendarEvents) plus the account/calendar registry (calendarOverview), themes entirely
// from the global tokens, and colours each source from the categorical source palette. This PR ships
// the shared chrome + the Agenda body; Month/Week/Day/Year land in the next PR (the view switcher
// grows with AVAILABLE_VIEWS). Nothing here writes back to any provider.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { calendarOverview, listAllCalendarEvents, syncCalendar } from "../../lib/ipc";
import type { CalendarEvent, CalendarOverview } from "../../lib/types";
import {
  readHidden,
  readView,
  writeHidden,
  writeView,
  type CalendarViewMode,
} from "../../lib/calendarPrefs";
import { sourceColors, useTheme } from "../../theme";
import { Skeleton } from "../ui";
import { CalendarHeader } from "./CalendarHeader";
import { AgendaView } from "./views/AgendaView";

// Views wired in this PR. PR2 adds "month" / "week" / "day" / "year"; the switcher shows only these,
// and readView() clamps a persisted value to what's available so an older/newer build never lands on
// a missing view.
const AVAILABLE_VIEWS: readonly CalendarViewMode[] = ["agenda"];
const DEFAULT_VIEW: CalendarViewMode = "agenda";

// Last-good data, kept in module scope so returning to the tab doesn't flash an empty grid before the
// mirror re-reads (mirrors FocusView's cache).
let cachedEvents: CalendarEvent[] = [];
let cachedOverview: CalendarOverview | null = null;

export function CalendarView() {
  const { system, accent } = useTheme();
  const [overview, setOverview] = useState<CalendarOverview | null>(() => cachedOverview);
  const [events, setEvents] = useState<CalendarEvent[]>(() => cachedEvents);
  const [hidden, setHidden] = useState<Set<string>>(() => readHidden());
  const [view, setView] = useState<CalendarViewMode>(() => readView(AVAILABLE_VIEWS, DEFAULT_VIEW));
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

  const onPrev = useCallback(
    () => setCursor((c) => new Date(c.getFullYear(), c.getMonth() - 1, 1)),
    [],
  );
  const onNext = useCallback(
    () => setCursor((c) => new Date(c.getFullYear(), c.getMonth() + 1, 1)),
    [],
  );
  const onToday = useCallback(() => setCursor(new Date()), []);

  const colorOf = useMemo(() => {
    const ids = overview?.calendars.map((c) => c.id) ?? [];
    const map = sourceColors(ids, system, accent);
    return (calendarId: string) => map.get(calendarId) ?? "var(--ink4)";
  }, [overview, system, accent]);

  const visibleEvents = useMemo(
    () => events.filter((e) => !hidden.has(e.calendar_id)),
    [events, hidden],
  );

  const label = cursor.toLocaleDateString(undefined, { month: "long", year: "numeric" });

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
    <div className="flex h-full flex-1 flex-col">
      <CalendarHeader
        view={view}
        availableViews={AVAILABLE_VIEWS}
        onViewChange={onViewChange}
        label={label}
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
      ) : (
        <AgendaView events={visibleEvents} fromDay={cursor} colorOf={colorOf} />
      )}
    </div>
  );
}
