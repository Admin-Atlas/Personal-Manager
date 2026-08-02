// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The unified, read-only multi-calendar aggregator tab (card 8). Reads the widened mirror
// (listAllCalendarEvents) plus the account/calendar registry (calendarOverview), themes entirely
// from the global tokens, and colours each source from the categorical source palette. Slate/Editorial
// render the pixel grids (Day/Week time-grid, Month, Year) + Agenda; Terminal forks to a mono, flat set
// (a CLI status strip + agenda/tables, never a pixel grid) enumerated explicitly per view so nothing
// falls through to the wrong body. A neutral hint flags paging past the synced band; each view fades up
// on switch (respecting prefers-reduced-motion); ←/→/t drive navigation. Synced events are read-only;
// the only interactive elements are the two first-party overlays — project milestones (click opens
// their project) and freeform pinboard timeline entries (click opens the Pinboard) — each an all-day
// event in its own hue, injected here and never written back to any calendar.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  calendarOverview,
  getPref,
  listAllCalendarEvents,
  listAllMilestones,
  syncCalendar,
} from "../../lib/ipc";
import type { CalendarEvent, CalendarOverview, Milestone } from "../../lib/types";
import {
  clampDayCount,
  readCursorDay,
  readDayCount,
  readHidden,
  readOpenOn,
  readRange,
  readRangeBounds,
  readView,
  readZones,
  writeCursorDay,
  writeDayCount,
  writeRange,
  writeRangeBounds,
  writeView,
  writeZones,
  type CalendarRange,
  type CalendarViewMode,
  type RangeBounds,
} from "../../lib/calendarPrefs";
import { useHorizontalWheelShift } from "../../lib/useHorizontalWheelShift";
import { resolveRangeBounds } from "../../lib/calendarGeom";
import { formatDateLocal } from "../../lib/format";
import {
  addDays,
  dayKey,
  eventDaySpan,
  isMilestoneEvent,
  isOverlayEvent,
  isPinboardEvent,
  MILESTONE_CALENDAR_ID,
  occurrenceKey,
  PINBOARD_CALENDAR_ID,
  startOfDay,
  startOfWeek,
} from "../../lib/calendar-layout";
import { pinboardEntries, type PinboardEntry } from "../../lib/pinboard/calendarEntries";
import { PINBOARD_PREF_KEY } from "../../lib/pinboard/types";
import {
  milestoneColor,
  pinboardColor,
  sourceColors,
  sourceShapeIndex,
  useTheme,
  useUserTime,
} from "../../theme";
import { Callout, Skeleton } from "../ui";
import { useNowTick } from "../../lib/useNowTick";
import { CalendarEventPopover } from "./parts/CalendarEventPopover";
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
let cachedMilestones: Milestone[] = [];
let cachedPinboard: PinboardEntry[] = [];

/** How many days the grid shows for a view. Week is 7 by definition; Day is the user's chosen
 *  width (1-6). Anything else isn't a day grid. */
function windowDays(view: CalendarViewMode, dayCount: number): number {
  if (view === "week") return 7;
  if (view === "day") return dayCount;
  return 0;
}

/** Step the cursor by one period in the current view's units. The day grids move by a WHOLE
 *  window — 7 for Week, N for an N-day Day view — so the arrows page rather than nudge; the
 *  sideways swipe is what moves a single day. */
function stepCursor(view: CalendarViewMode, cur: Date, dir: number, dayCount: number): Date {
  switch (view) {
    case "day":
      return addDays(cur, dir * windowDays(view, dayCount));
    case "week":
      return addDays(cur, 7 * dir);
    case "year":
      return new Date(cur.getFullYear() + dir, cur.getMonth(), 1);
    default: // month, agenda
      return new Date(cur.getFullYear(), cur.getMonth() + dir, 1);
  }
}

/** The period label shown between the nav arrows for each view. */
function viewLabel(view: CalendarViewMode, cur: Date, dayCount: number): string {
  switch (view) {
    case "day": {
      if (dayCount <= 1) {
        return `${cur.toLocaleDateString(undefined, { weekday: "long" })} ${formatDateLocal(cur)}`;
      }
      return `${formatDateLocal(cur)} – ${formatDateLocal(addDays(cur, dayCount - 1))}`;
    }
    case "week":
      // The window starts at the cursor, not at its Monday: the week is free to begin on any day
      // once you have swiped it sideways. "Today" is what snaps it back to a Monday.
      return `${formatDateLocal(cur)} – ${formatDateLocal(addDays(cur, 6))}`;
    case "year":
      return String(cur.getFullYear());
    default: // month, agenda
      return cur.toLocaleDateString(undefined, { month: "long", year: "numeric" });
  }
}

/** The visible date window for a view, used only to flag paging past the synced band. `end` is
 *  exclusive. Agenda is anchored/open-ended, so its window is just the anchor day — the hint then
 *  fires only when the anchor itself sits outside the mirror, not for a distant future event. */
function visibleRange(
  view: CalendarViewMode,
  cur: Date,
  dayCount: number,
): { start: Date; end: Date } {
  const day0 = startOfDay(cur);
  switch (view) {
    case "day":
      return { start: day0, end: addDays(day0, dayCount) };
    case "week":
      return { start: day0, end: addDays(day0, 7) };
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

interface CalendarViewProps {
  /** Open a project's page — wired only to the clickable milestone overlay events; the rest of the
   *  calendar stays read-only. */
  onOpenProject?: (project: string) => void;
  /** Open the Pinboard tab — wired to the pinboard overlay events, which have no project to open. */
  onOpenPinboard?: () => void;
}

export function CalendarView({ onOpenProject, onOpenPinboard }: CalendarViewProps) {
  const { system, accent, colorblind } = useTheme();
  const { coords } = useUserTime();
  const [overview, setOverview] = useState<CalendarOverview | null>(() => cachedOverview);
  const [events, setEvents] = useState<CalendarEvent[]>(() => cachedEvents);
  const [milestones, setMilestones] = useState<Milestone[]>(() => cachedMilestones);
  const [pinboardItems, setPinboardItems] = useState<PinboardEntry[]>(() => cachedPinboard);
  const [hidden, setHidden] = useState<Set<string>>(() => readHidden());
  const [view, setView] = useState<CalendarViewMode>(() => readView(AVAILABLE_VIEWS, DEFAULT_VIEW));
  const [range, setRange] = useState<CalendarRange>(() => readRange());
  // Extra gutter timezones + any custom Work/Day hour windows (both per-device view prefs).
  const [zones, setZones] = useState<string[]>(() => readZones());
  const [customBounds, setCustomBounds] = useState<Partial<Record<CalendarRange, RangeBounds>>>(
    () => readRangeBounds(),
  );
  // Where the calendar opens: today, or wherever it was left. Read once at mount — flipping the
  // setting later should change the NEXT open, not teleport the view out from under you.
  const [cursor, setCursor] = useState<Date>(() => {
    // `openOn` is the whole of "remember where I was", start day included: Week view's leftmost
    // column IS the cursor's day, so restoring the cursor restores the shape for free.
    if (readOpenOn() === "last") return readCursorDay() ?? new Date();
    // With remembering OFF, Week view opens on the ordinary Monday-to-Sunday week containing today —
    // the same shape `Today` snaps to. #558 dropped this snap from the seed while making the window
    // day-steppable and replaced it with nothing, so the seed fell through to a bare `new Date()`
    // and the leftmost column became *today* on every mount. Restoring the snap is the fix; a
    // remembered start day is NOT, because it re-shapes the week for someone who has switched
    // remembering off (see calendarPrefs' note on the deleted `pm.calendar.weekStart`).
    if (view === "week") return startOfWeek(new Date());
    return new Date();
  });
  // How wide the Day view is (1-6). Week is always 7.
  const [dayCount, setDayCount] = useState<number>(readDayCount);
  // Persist the cursor on every MOVE, whatever the openOn setting says — so turning "where I left
  // off" on works from that moment rather than only after the next navigation.
  //
  // Skipping the first run matters: this effect also fires on mount, and in the default
  // `openOn: 'today'` mode the mount value is simply today — so merely opening the Calendar tab
  // stamped today over the day the user had actually left, and "where I left off" could only ever
  // restore the last tab visit rather than the last place they navigated to.
  const cursorWritten = useRef(false);
  useEffect(() => {
    if (!cursorWritten.current) {
      cursorWritten.current = true;
      return;
    }
    writeCursorDay(cursor);
  }, [cursor]);
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

  const loadMilestones = useCallback(async () => {
    try {
      const ms = await listAllMilestones();
      if (!aliveRef.current) return;
      setMilestones(ms);
      cachedMilestones = ms;
    } catch {
      // Keep the last-good milestones; a read failure is transient and the overlay is non-critical.
    }
  }, []);

  // The board lives in the encrypted settings table (not localStorage), so its overlay is a plain read
  // like everything else here. Switching tabs unmounts this view, so the mount load below picks up any
  // board edit; the focus listener covers one made while the window was away.
  const loadPinboard = useCallback(async () => {
    try {
      const raw = await getPref(PINBOARD_PREF_KEY);
      if (!aliveRef.current) return;
      const entries = pinboardEntries(raw);
      setPinboardItems(entries);
      cachedPinboard = entries;
    } catch {
      // Keep the last-good entries; a read failure is transient and the overlay is non-critical.
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
    void loadMilestones();
    void loadPinboard();
    // Re-read EVERYTHING on focus — events, overview, milestones AND the board. The background poll can
    // have shifted the synced band, added a calendar, or changed the last-sync time while we were
    // away, and milestone/board edits happen in other tabs (which unmount this one), so refreshing
    // events alone would leave the range hint, source colours, "synced" label, and overlays stale.
    const onFocus = () => {
      void loadEvents();
      void loadOverview();
      void loadMilestones();
      void loadPinboard();
    };
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [loadOverview, loadEvents, loadMilestones, loadPinboard]);

  const onRefresh = useCallback(async () => {
    setSyncing(true);
    setError(null);
    try {
      await syncCalendar();
      await Promise.all([loadEvents(), loadOverview(), loadMilestones(), loadPinboard()]);
    } catch (e) {
      if (aliveRef.current) setError(e instanceof Error ? e.message : String(e));
    } finally {
      if (aliveRef.current) setSyncing(false);
    }
  }, [loadEvents, loadOverview, loadMilestones, loadPinboard]);

  // The visibility TOGGLES moved to the sidebar block (one control, one home), so this view only
  // reads the set — but the sidebar is mounted right beside it, so a tick there has to land here
  // without a remount. writeHidden announces on the app-wide signal; follow it.
  useEffect(() => {
    const sync = () => setHidden(readHidden());
    window.addEventListener("pm:settings-changed", sync);
    return () => window.removeEventListener("pm:settings-changed", sync);
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

  const onPrev = useCallback(
    () => setCursor((c) => stepCursor(view, c, -1, dayCount)),
    [view, dayCount],
  );
  const onNext = useCallback(
    () => setCursor((c) => stepCursor(view, c, 1, dayCount)),
    [view, dayCount],
  );
  // Today re-snaps as well as re-dates. After swiping the week onto a Wednesday start, "Today"
  // should give back the ordinary Monday-Sunday week that contains today, not a Wednesday one that
  // happens to include it — otherwise there is no way back to the conventional grid.
  const onToday = useCallback(
    () => setCursor(view === "week" ? startOfWeek(new Date()) : new Date()),
    [view],
  );
  // One day per step, in the direction of travel: a swipe left (positive deltaX) moves forward.
  // Both gestures move the cursor and nothing else — `writeCursorDay` below is what remembers it,
  // and `openOn` decides whether that memory is ever read back.
  const gridRef = useHorizontalWheelShift(
    (days) => setCursor((c) => addDays(c, days)),
    view === "day" || view === "week",
  );
  const onPickDate = useCallback((d: Date) => setCursor(d), []);
  // Drilling from the Year view: jump to that day and open the Day view.
  const onSelectYearDay = useCallback(
    (d: Date) => {
      setCursor(d);
      onViewChange("day");
    },
    [onViewChange],
  );
  // The scrolling Month/Year views report the period filling the pane so the header label tracks the
  // scroll. The views guard against re-scrolling on the cursor change they just caused (no loop).
  const onFocusDate = useCallback((d: Date) => setCursor(d), []);

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
    const map = sourceColors(ids, system, accent, colorblind);
    return (calendarId: string) => {
      if (calendarId === MILESTONE_CALENDAR_ID) return milestoneColor(system);
      if (calendarId === PINBOARD_CALENDAR_ID) return pinboardColor(system);
      return map.get(calendarId) ?? "var(--ink4)";
    };
  }, [overview, system, accent, colorblind]);

  // The per-source SHAPE slot, parallel to colorOf, for the colour-blind axis's redundant dot shapes.
  // Only real calendars get a slot; overlays (milestones/pinboard) return undefined → plain circle,
  // and they already carry their own distinct hues. Keyed only on the calendar set (like sourceColors).
  const shapeOf = useMemo(() => {
    const slots = sourceShapeIndex(overview?.calendars.map((c) => c.id) ?? []);
    return (calendarId: string) => slots.get(calendarId);
  }, [overview]);

  // Project milestones as synthetic all-day events — a first-party overlay, not synced. Only draw a
  // milestone that ISN'T already on the calendar as a real event: PM-native (unlinked) ones, plus
  // linked ones whose event is `event_missing` (out of the mirror band / deselected / deleted), which
  // nothing else shows. A linked, in-mirror milestone is already drawn as its real event, so skip it
  // to avoid a double. (Accepted edge: a linked, in-mirror milestone whose calendar the user hid then
  // shows nowhere.) Dateless milestones have no day to sit on, so they're excluded.
  const milestoneEvents = useMemo<CalendarEvent[]>(() => {
    const out: CalendarEvent[] = [];
    for (const m of milestones) {
      if (!m.due_date) continue;
      if (!(m.event_uid === null || m.event_missing)) continue;
      out.push({
        id: `milestone:${m.id}`,
        calendar_id: MILESTONE_CALENDAR_ID,
        // Suffix the owning project so two milestones with similar labels in different projects are
        // distinguishable on the all-day row (a milestone always belongs to exactly one project).
        summary:
          m.state === "met" ? `✓ ${m.label} · ${m.project_name}` : `${m.label} · ${m.project_name}`,
        description: null,
        location: null,
        start: m.due_date.slice(0, 10),
        end: null,
        all_day: true,
        html_link: null,
        uid: null,
      });
    }
    return out;
  }, [milestones]);

  // Freeform pinboard timeline entries as synthetic all-day events — the second first-party overlay.
  // `pinboardEntries` has already dropped project-bound timelines (their entries are real milestones,
  // drawn above), opted-out widgets, and dateless entries, so this is a straight projection.
  const pinboardEvents = useMemo<CalendarEvent[]>(
    () =>
      pinboardItems.map((e) => ({
        id: `pinboard:${e.widgetId}:${e.itemId}`,
        calendar_id: PINBOARD_CALENDAR_ID,
        summary: e.label,
        description: null,
        location: null,
        start: e.date,
        end: null,
        all_day: true,
        html_link: null,
        uid: null,
      })),
    [pinboardItems],
  );

  const visibleEvents = useMemo(() => {
    // Filter to visible calendars, THEN dedup the same physical OCCURRENCE mirrored on two of them
    // (same iCal UID *and* start), keeping the first visible copy — otherwise it renders twice in the
    // grid/agenda. Done after the hide filter so hiding one calendar still shows the copy on the
    // calendar left visible. Keyed on the UID alone this also collapsed every recurring series to one
    // occurrence across the whole mirror — see `occurrenceKey`, which FocusUpcoming shares.
    const seen = new Set<string>();
    const out: CalendarEvent[] = [];
    for (const e of events) {
      if (hidden.has(e.calendar_id)) continue;
      const key = occurrenceKey(e);
      if (key !== null) {
        if (seen.has(key)) continue;
        seen.add(key);
      }
      out.push(e);
    }
    // Both first-party overlays ride on top, each toggleable like a calendar via the same `hidden` set.
    // Their events carry a null uid + a unique calendar_id, so they skip the dedup and hide filter
    // cleanly.
    if (!hidden.has(MILESTONE_CALENDAR_ID)) out.push(...milestoneEvents);
    if (!hidden.has(PINBOARD_CALENDAR_ID)) out.push(...pinboardEvents);
    return out;
  }, [events, hidden, milestoneEvents, pinboardEvents]);

  // The event popup that's open, anchored at the clicked element's rect (null = closed).
  const [eventPopup, setEventPopup] = useState<{ ev: CalendarEvent; anchor: DOMRect } | null>(null);

  // Clicking an event: a PM overlay keeps its jump (a milestone opens its project, a freeform pinboard
  // entry opens the Pinboard), while a real synced event opens the in-place detail popup anchored at
  // the click. Every event is now clickable (was overlay-only).
  const onEventClick = useCallback(
    (ev: CalendarEvent, anchor: DOMRect) => {
      if (isPinboardEvent(ev)) {
        onOpenPinboard?.();
        return;
      }
      if (isMilestoneEvent(ev)) {
        const id = Number(ev.id.slice("milestone:".length));
        const m = milestones.find((x) => x.id === id);
        if (m) onOpenProject?.(m.project_name);
        return;
      }
      setEventPopup({ ev, anchor });
    },
    [milestones, onOpenProject, onOpenPinboard],
  );

  // Both day grids are the same thing now — N consecutive days starting at the cursor. Week is
  // simply N=7, which is why it can start on a Wednesday after a swipe.
  const gridDays = useMemo<Date[]>(() => {
    const n = windowDays(view, dayCount);
    if (n === 0) return [];
    const first = startOfDay(cursor);
    return Array.from({ length: n }, (_, i) => addDays(first, i));
  }, [view, cursor, dayCount]);

  // The visible-hour window the time grid frames: a custom Work/Day override, else the computed
  // default (Work 08:30–17:30, Day = local sunrise/sunset, 24h = full). Recomputed with the cursor so
  // the Day default tracks the shown date's daylight (rounded to the hour, it shifts ~seasonally).
  const activeBounds = useMemo(
    () => resolveRangeBounds(range, customBounds, coords, cursor),
    [range, customBounds, coords, cursor],
  );

  const label = viewLabel(view, cursor, dayCount);
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
    const { start, end } = visibleRange(view, cursor, dayCount);
    const bandStart = startOfDay(ms).getTime();
    const bandEnd = startOfDay(me).getTime();
    const firstVisible = startOfDay(start).getTime();
    const lastVisible = startOfDay(addDays(end, -1)).getTime();
    if (firstVisible >= bandStart && lastVisible <= bandEnd) return null;
    return `Outside the synced range — only ${formatDateLocal(ms)} – ${formatDateLocal(me)} is mirrored.`;
  }, [overview, view, cursor, dayCount]);

  // The count the Terminal chrome strip shows at Power depth. For the bounded grids it's the events
  // touching the visible period (so it matches what's on screen, not the whole −1..+13-month mirror);
  // for the open-ended agenda it's everything from the anchor day forward.
  const chromeCount = useMemo(() => {
    if (view === "agenda") {
      const fromMs = startOfDay(cursor).getTime();
      return visibleEvents.filter((e) => {
        if (isOverlayEvent(e)) return false; // count synced events only, honest to "N events"
        const span = eventDaySpan(e);
        return span && span.endDay.getTime() >= fromMs;
      }).length;
    }
    const { start, end } = visibleRange(view, cursor, dayCount);
    const startMs = start.getTime();
    const endMs = end.getTime(); // exclusive
    return visibleEvents.filter((e) => {
      if (isOverlayEvent(e)) return false; // count synced events only, honest to "N events"
      const span = eventDaySpan(e);
      return span && span.startDay.getTime() < endMs && span.endDay.getTime() >= startMs;
    }).length;
  }, [visibleEvents, view, cursor, dayCount]);

  if (loading && !overview) {
    return (
      <div className="flex flex-1 flex-col gap-2 p-4">
        <Skeleton className="h-8 w-full" />
        <Skeleton className="h-full w-full" />
      </div>
    );
  }

  const hasCalendars = (overview?.calendars.length ?? 0) > 0;
  // Both first-party overlays exist WITHOUT any connected calendar, so the grid has to render for them
  // alone — otherwise a user with milestones or pinboard entries and no account is told "No calendars
  // connected yet." over data the Calendars menu is simultaneously offering to show/hide. Read the raw
  // overlay memos, not `visibleEvents`: hiding an overlay should empty the grid, never swap the whole
  // body for an onboarding message. The empty state is then honest — it means nothing to draw at all.
  const hasOverlay = milestoneEvents.length > 0 || pinboardEvents.length > 0;

  // The active body for the current System + view. Terminal forks to the mono set and never mounts the
  // pixel TimeGridView; both branches enumerate all five views so none falls through to the wrong body.
  const renderBody = () => {
    if (isTerminal) {
      switch (view) {
        case "month":
          return (
            <TerminalMonthTable
              cursor={cursor}
              events={visibleEvents}
              colorOf={colorOf}
              now={now}
              onEventClick={onEventClick}
            />
          );
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
          return (
            <TerminalAgenda
              events={visibleEvents}
              colorOf={colorOf}
              days={gridDays}
              now={now}
              onEventClick={onEventClick}
            />
          );
        case "agenda":
        default:
          return (
            <TerminalAgenda
              events={visibleEvents}
              colorOf={colorOf}
              fromDay={cursor}
              now={now}
              onEventClick={onEventClick}
            />
          );
      }
    }
    switch (view) {
      case "agenda":
        return (
          <AgendaView
            events={visibleEvents}
            fromDay={cursor}
            colorOf={colorOf}
            now={now}
            onEventClick={onEventClick}
          />
        );
      case "month":
        return (
          <MonthView
            cursor={cursor}
            events={visibleEvents}
            colorOf={colorOf}
            shapeOf={shapeOf}
            onFocusDate={onFocusDate}
            now={now}
            onEventClick={onEventClick}
          />
        );
      case "year":
        return (
          <YearView
            cursor={cursor}
            events={visibleEvents}
            onSelectDay={onSelectYearDay}
            onFocusDate={onFocusDate}
          />
        );
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
            now={now}
            onEventClick={onEventClick}
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
        dayCount={dayCount}
        onDayCountChange={(n) => {
          setDayCount(clampDayCount(n));
          writeDayCount(n);
        }}
        customBounds={customBounds}
        onBoundsChange={onBoundsChange}
        coords={coords}
        label={label}
        cursor={cursor}
        onPickDate={onPickDate}
        onPrev={onPrev}
        onNext={onNext}
        onToday={onToday}
        onRefresh={onRefresh}
        syncing={syncing}
        lastSync={overview?.last_sync ?? null}
      />

      {isTerminal && <TerminalChrome view={view} label={label} count={chromeCount} />}

      {error && (
        <Callout variant="strip" size="md" className="font-ui">
          {error}
        </Callout>
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

      {/* Overlays but no connected calendar: the grid below is real (milestones / pinboard entries), so
          the connect-a-calendar nudge demotes to a hint strip rather than replacing the body. */}
      {!hasCalendars && hasOverlay && (
        <div className="border-b border-rule bg-panel px-4 py-1.5 text-center font-mono text-xs text-ink4">
          No calendars connected — showing your milestones and pinboard entries only.
        </div>
      )}

      {!hasCalendars && !hasOverlay ? (
        <div className="flex flex-1 flex-col items-center justify-center gap-2 p-8 text-center">
          <p className="text-sm text-ink2">No calendars connected yet.</p>
          <p className="max-w-sm text-xs text-ink4">
            Connect a Google, Outlook, or iCal calendar in Settings → Connectors, then it appears
            here read-only.
          </p>
        </div>
      ) : (
        // The swipe target. `ref={gridRef}` was missing entirely until now, so the Calendar tab's
        // horizontal swipe had never once fired despite the v3.88.0 notes advertising it here
        // alongside Focus's. Note WHERE this branch sits: it renders only once `overview` has
        // resolved, and the day columns inside it are keyed by date, so they are all replaced on
        // every step. `useHorizontalWheelShift` owns neither problem from here — it listens on the
        // window and hit-tests this element's box, so it needs only to be told which box (its
        // header has the two failure modes that shape came from).
        <div ref={gridRef} className="flex min-h-0 flex-1 flex-col">
          {/* key restarts the 0.25s fade-up on view switch; under prefers-reduced-motion the
              keyframe name doesn't resolve, so this is a no-op (the motion lives in index.css, not
              JS). The day component of the key (from the minute tick) remounts the body when the
              date rolls over at midnight, so every view's "today" highlight advances without an
              interaction. */}
          <div
            key={`${view}:${dayKey(startOfDay(now))}`}
            className="flex min-h-0 flex-1 flex-col"
            style={{ animation: "pm-fade-up 0.25s ease-out" }}
          >
            {renderBody()}
          </div>
        </div>
      )}

      {eventPopup && (
        <CalendarEventPopover
          event={eventPopup.ev}
          anchor={eventPopup.anchor}
          calendar={overview?.calendars.find((c) => c.id === eventPopup.ev.calendar_id) ?? null}
          color={colorOf(eventPopup.ev.calendar_id)}
          milestone={
            eventPopup.ev.uid
              ? (milestones.find((m) => m.event_uid && m.event_uid === eventPopup.ev.uid) ?? null)
              : null
          }
          onClose={() => setEventPopup(null)}
          onOpenProject={
            onOpenProject
              ? (p) => {
                  setEventPopup(null);
                  onOpenProject(p);
                }
              : undefined
          }
        />
      )}
    </div>
  );
}
