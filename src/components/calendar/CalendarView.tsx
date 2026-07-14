// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The unified, read-only multi-calendar aggregator tab (card 8). Reads the widened mirror
// (listAllCalendarEvents) plus the account/calendar registry (calendarOverview), themes entirely
// from the global tokens, and colours each source from the categorical source palette. Slate/Editorial
// render the pixel grids (Day/Week time-grid, Month, Year) + Agenda; Terminal forks to a mono, flat set
// (a CLI status strip + agenda/tables, never a pixel grid) enumerated explicitly per view so nothing
// falls through to the wrong body. A neutral hint flags paging past the synced band; each view fades up
// on switch (respecting prefers-reduced-motion); ←/→/t drive navigation. Nothing here writes back.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { calendarOverview, listAllCalendarEvents, syncCalendar } from "../../lib/ipc";
import type { CalendarEvent, CalendarOverview } from "../../lib/types";
import {
  readHidden,
  readRange,
  readRangeBounds,
  readView,
  readZones,
  writeHidden,
  writeRange,
  writeRangeBounds,
  writeView,
  writeZones,
  type CalendarRange,
  type CalendarViewMode,
  type RangeBounds,
} from "../../lib/calendarPrefs";
import { resolveRangeBounds } from "../../lib/calendarGeom";
import { formatDateLocal } from "../../lib/format";
import { addDays, dayKey, eventDaySpan, startOfDay } from "../../lib/calendar-layout";
import { sourceColors, useTheme, useUserTime } from "../../theme";
import { Skeleton } from "../ui";
import { useNowTick } from "./parts/useNowTick";
import { CalendarHeader } from "./CalendarHeader";
import { AgendaView } from "./views/AgendaView";
import { TimeGridView } from "./views/TimeGridView";
import { MonthView } from "./views/MonthView";
import { YearView } from "./views/YearView";
import { TerminalChrome } from "./terminal/TerminalChrome";
import { TerminalAgenda } from "./terminal/TerminalAgenda";
import { TerminalMonthTable } from "./terminal/TerminalMonthTable";
import { TerminalYearTable } from "./terminal/TerminalYearTable";

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

/** The visible date window for a view, used only to flag paging past the synced band. `end` is
 *  exclusive. Agenda is anchored/open-ended, so its window is just the anchor day — the hint then
 *  fires only when the anchor itself sits outside the mirror, not for a distant future event. */
function visibleRange(view: CalendarViewMode, cur: Date): { start: Date; end: Date } {
  const day0 = startOfDay(cur);
  switch (view) {
    case "day":
      return { start: day0, end: addDays(day0, 1) };
    case "week": {
      const monday = startOfWeek(cur);
      return { start: monday, end: addDays(monday, 7) };
    }
    case "month":
      return {
        start: new Date(cur.getFullYear(), cur.getMonth(), 1),
        end: new Date(cur.getFullYear(), cur.getMonth() + 1, 1),
      };
    case "year":
      return {
        start: new Date(cur.getFullYear(), 0, 1),
        end: new Date(cur.getFullYear() + 1, 0, 1),
      };
    default: // agenda
      return { start: day0, end: addDays(day0, 1) };
  }
}

export function CalendarView() {
  const { system, accent } = useTheme();
  const { coords } = useUserTime();
  const [overview, setOverview] = useState<CalendarOverview | null>(() => cachedOverview);
  const [events, setEvents] = useState<CalendarEvent[]>(() => cachedEvents);
  const [hidden, setHidden] = useState<Set<string>>(() => readHidden());
  const [view, setView] = useState<CalendarViewMode>(() => readView(AVAILABLE_VIEWS, DEFAULT_VIEW));
  const [range, setRange] = useState<CalendarRange>(() => readRange());
  // Extra gutter timezones + any custom Work/Day hour windows (both per-device view prefs).
  const [zones, setZones] = useState<string[]>(() => readZones());
  const [customBounds, setCustomBounds] = useState<Partial<Record<CalendarRange, RangeBounds>>>(
    () => readRangeBounds(),
  );
  const [cursor, setCursor] = useState<Date>(() => new Date());
  const [loading, setLoading] = useState(cachedOverview === null);
  const [syncing, setSyncing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [eventsFailed, setEventsFailed] = useState(false);
  const aliveRef = useRef(true);
  // Monotonic token so an older in-flight events read (e.g. a focus refresh) can't overwrite a newer
  // one that resolved first.
  const eventsSeqRef = useRef(0);
  const now = useNowTick();

  useEffect(() => {
    aliveRef.current = true;
    return () => {
      aliveRef.current = false;
    };
  }, []);

  const loadEvents = useCallback(async () => {
    const seq = ++eventsSeqRef.current;
    try {
      const evts = await listAllCalendarEvents();
      if (!aliveRef.current || seq !== eventsSeqRef.current) return;
      setEvents(evts);
      cachedEvents = evts;
      setEventsFailed(false);
    } catch {
      // Keep the last-good events; a read failure is transient. Flag it only when we have nothing to
      // show, so the empty grid isn't mistaken for "no events".
      if (aliveRef.current && seq === eventsSeqRef.current && cachedEvents.length === 0) {
        setEventsFailed(true);
      }
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
    // Re-read the WHOLE mirror on focus — both events and the overview. The background poll can have
    // shifted the synced band, added a calendar, or changed the last-sync time while we were away, so
    // refreshing events alone would leave the range hint, source colours, and "synced" label stale.
    const onFocus = () => {
      void loadEvents();
      void loadOverview();
    };
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

  const onZonesChange = useCallback((next: string[]) => {
    setZones(next);
    writeZones(next);
  }, []);

  const onBoundsChange = useCallback((r: CalendarRange, bounds: RangeBounds | null) => {
    setCustomBounds((prev) => {
      const next = { ...prev };
      if (bounds) next[r] = bounds;
      else delete next[r];
      writeRangeBounds(next);
      return next;
    });
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

  // Keyboard nav while the tab is mounted: ← / → step the period, `t` jumps to today. Ignored while a
  // field is focused or a modifier is held (so app shortcuts and text entry are untouched).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.metaKey || e.ctrlKey || e.altKey) return;
      const t = e.target as HTMLElement | null;
      if (t && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName))) return;
      if (e.key === "ArrowLeft") {
        e.preventDefault();
        onPrev();
      } else if (e.key === "ArrowRight") {
        e.preventDefault();
        onNext();
      } else if (e.key === "t" || e.key === "T") {
        onToday();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onPrev, onNext, onToday]);

  const colorOf = useMemo(() => {
    const ids = overview?.calendars.map((c) => c.id) ?? [];
    const map = sourceColors(ids, system, accent);
    return (calendarId: string) => map.get(calendarId) ?? "var(--ink4)";
  }, [overview, system, accent]);

  const visibleEvents = useMemo(() => {
    // Filter to visible calendars, THEN dedup the same physical event mirrored on two of them (same
    // iCal UID), keeping the first visible copy — otherwise it renders twice in the grid/agenda. Done
    // after the hide filter so hiding one calendar still shows the copy on the calendar left visible.
    const seen = new Set<string>();
    const out: CalendarEvent[] = [];
    for (const e of events) {
      if (hidden.has(e.calendar_id)) continue;
      if (e.uid) {
        if (seen.has(e.uid)) continue;
        seen.add(e.uid);
      }
      out.push(e);
    }
    return out;
  }, [events, hidden]);

  const gridDays = useMemo<Date[]>(() => {
    if (view === "day") return [startOfDay(cursor)];
    if (view === "week") {
      const monday = startOfWeek(cursor);
      return Array.from({ length: 7 }, (_, i) => addDays(monday, i));
    }
    return [];
  }, [view, cursor]);

  // The visible-hour window the time grid frames: a custom Work/Day override, else the computed
  // default (Work 08:30–17:30, Day = local sunrise/sunset, 24h = full). Recomputed with the cursor so
  // the Day default tracks the shown date's daylight (rounded to the hour, it shifts ~seasonally).
  const activeBounds = useMemo(
    () => resolveRangeBounds(range, customBounds, coords, cursor),
    [range, customBounds, coords, cursor],
  );

  const label = viewLabel(view, cursor);
  const isTerminal = system === "terminal";

  // "Outside the synced range" hint: fires when the visible window falls before mirror_start or after
  // mirror_end (the mirror is a fixed −1…+13-month band, so paging far enough leaves it). Neutral, not
  // an error — the band is by design. Today/now logic elsewhere never assumes today is inside the band.
  const rangeHint = useMemo(() => {
    if (!overview) return null;
    const ms = new Date(overview.mirror_start);
    const me = new Date(overview.mirror_end);
    if (Number.isNaN(ms.getTime()) || Number.isNaN(me.getTime())) return null;
    // Compare on LOCAL calendar days, not raw instants: the band bounds are UTC-ish while the visible
    // window is built from local midnights, so a sub-day tz offset at a band edge would otherwise trip
    // the hint on a month whose every displayed day is actually in-band. `end` is exclusive → last
    // visible day is end−1.
    const { start, end } = visibleRange(view, cursor);
    const bandStart = startOfDay(ms).getTime();
    const bandEnd = startOfDay(me).getTime();
    const firstVisible = startOfDay(start).getTime();
    const lastVisible = startOfDay(addDays(end, -1)).getTime();
    if (firstVisible >= bandStart && lastVisible <= bandEnd) return null;
    return `Outside the synced range — only ${formatDateLocal(ms)} – ${formatDateLocal(me)} is mirrored.`;
  }, [overview, view, cursor]);

  // The count the Terminal chrome strip shows at Power depth. For the bounded grids it's the events
  // touching the visible period (so it matches what's on screen, not the whole −1..+13-month mirror);
  // for the open-ended agenda it's everything from the anchor day forward.
  const chromeCount = useMemo(() => {
    if (view === "agenda") {
      const fromMs = startOfDay(cursor).getTime();
      return visibleEvents.filter((e) => {
        const span = eventDaySpan(e);
        return span && span.endDay.getTime() >= fromMs;
      }).length;
    }
    const { start, end } = visibleRange(view, cursor);
    const startMs = start.getTime();
    const endMs = end.getTime(); // exclusive
    return visibleEvents.filter((e) => {
      const span = eventDaySpan(e);
      return span && span.startDay.getTime() < endMs && span.endDay.getTime() >= startMs;
    }).length;
  }, [visibleEvents, view, cursor]);

  if (loading && !overview) {
    return (
      <div className="flex flex-1 flex-col gap-2 p-4">
        <Skeleton className="h-8 w-full" />
        <Skeleton className="h-full w-full" />
      </div>
    );
  }

  const hasCalendars = (overview?.calendars.length ?? 0) > 0;

  // The active body for the current System + view. Terminal forks to the mono set and never mounts the
  // pixel TimeGridView; both branches enumerate all five views so none falls through to the wrong body.
  const renderBody = () => {
    if (isTerminal) {
      switch (view) {
        case "month":
          return <TerminalMonthTable cursor={cursor} events={visibleEvents} colorOf={colorOf} />;
        case "year":
          return (
            <TerminalYearTable
              cursor={cursor}
              events={visibleEvents}
              onSelectDay={onSelectYearDay}
            />
          );
        case "day":
        case "week":
          return <TerminalAgenda events={visibleEvents} colorOf={colorOf} days={gridDays} />;
        case "agenda":
        default:
          return <TerminalAgenda events={visibleEvents} colorOf={colorOf} fromDay={cursor} />;
      }
    }
    switch (view) {
      case "agenda":
        return <AgendaView events={visibleEvents} fromDay={cursor} colorOf={colorOf} />;
      case "month":
        return <MonthView cursor={cursor} events={visibleEvents} colorOf={colorOf} />;
      case "year":
        return <YearView cursor={cursor} events={visibleEvents} onSelectDay={onSelectYearDay} />;
      case "day":
      case "week":
      default:
        return (
          <TimeGridView
            days={gridDays}
            events={visibleEvents}
            colorOf={colorOf}
            range={range}
            bounds={activeBounds}
            zones={zones}
            onZonesChange={onZonesChange}
          />
        );
    }
  };

  return (
    <div className="flex h-full min-h-0 flex-1 flex-col" data-help="calendar-view">
      <CalendarHeader
        view={view}
        availableViews={AVAILABLE_VIEWS}
        onViewChange={onViewChange}
        range={range}
        onRangeChange={onRangeChange}
        customBounds={customBounds}
        onBoundsChange={onBoundsChange}
        coords={coords}
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

      {isTerminal && <TerminalChrome view={view} label={label} count={chromeCount} />}

      {error && (
        <div
          className="border-b border-rule px-4 py-2 font-ui text-sm text-[var(--st-due)]"
          style={{ background: "color-mix(in oklab, var(--st-due) 15%, transparent)" }}
        >
          {error}
        </div>
      )}

      {eventsFailed && !error && (
        <div className="border-b border-rule bg-panel px-4 py-1.5 text-center font-mono text-xs text-ink4">
          Couldn't load events — try Refresh.
        </div>
      )}

      {rangeHint && (
        <div className="border-b border-rule bg-panel px-4 py-1.5 text-center font-mono text-xs text-ink4">
          {rangeHint}
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
        // key restarts the 0.25s fade-up on view switch; under prefers-reduced-motion the keyframe
        // name doesn't resolve, so this is a no-op (the motion lives in index.css, not JS). The day
        // component of the key (from the minute tick) remounts the body when the date rolls over at
        // midnight, so every view's "today" highlight advances without an interaction.
        <div
          key={`${view}:${dayKey(startOfDay(now))}`}
          className="flex min-h-0 flex-1 flex-col"
          style={{ animation: "pm-fade-up 0.25s ease-out" }}
        >
          {renderBody()}
        </div>
      )}
    </div>
  );
}
