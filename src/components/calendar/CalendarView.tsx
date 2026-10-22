// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The unified multi-calendar aggregator tab (card 8). Reads the widened mirror
// (listAllCalendarEvents) plus the account/calendar registry (calendarOverview), themes entirely
// from the global tokens, and colours each source from the categorical source palette. Slate/Editorial
// render the pixel grids (Day/Week time-grid, Month, Year) + Agenda; Terminal forks to a mono, flat set
// (a CLI status strip + agenda/tables, never a pixel grid) enumerated explicitly per view so nothing
// falls through to the wrong body. A neutral hint flags paging past the synced band; each view fades up
// on switch (respecting prefers-reduced-motion); ←/→/t drive navigation. A synced event opens its
// detail popover, where a Google event PM may change offers Edit and Delete (#884; every change goes
// through `edit/useEventWrites`); "+ New event" and `c` open the editor for a new one, on a calendar
// the user picks, and Shift+C a quick box under the button, whose draft is drawn on the grid as it's
// typed (C10); everything else stays read-only. The two first-party overlays — project
// milestones (click opens their project) and freeform pinboard timeline entries (click opens the
// Pinboard) — are each an all-day event in its own hue, injected here and never written back to any
// calendar.

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { flushSync } from "react-dom";
import {
  calendarOverview,
  getPref,
  listAllCalendarEvents,
  listAllMilestones,
  onCalendarSynced,
  onCalendarWriteLanded,
  syncCalendar,
} from "../../lib/ipc";
import type { CalendarEvent, CalendarOverview, Milestone } from "../../lib/types";
import { reasonText } from "../../lib/calendarEdit/editReasons";
import { guestSuggestions } from "../../lib/calendarEdit/guests";
import {
  createdWhere,
  createReasonFor,
  createTargets,
  defaultTarget,
  lengthFor,
} from "../../lib/calendarEdit/createTargets";
import { newEventSeed, type EditorFields } from "../../lib/calendarEdit/eventDraft";
import {
  clampDayCount,
  readCursorDay,
  readDayCount,
  readHidden,
  readLastCreate,
  readOpenOn,
  readRange,
  readRangeBounds,
  readView,
  readZones,
  swipeDays,
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
import { calendarNavKey, calendarUndoKey } from "../../lib/calendarNavKey";
import { isAnyDialogOpen } from "../../lib/useDialogLayer";
import { resolveRangeBounds } from "../../lib/calendarGeom";
import { formatDateLocal } from "../../lib/format";
import {
  addDays,
  dayKey,
  draftEvent,
  eventDaySpan,
  ghostAfterRead,
  isDraftEvent,
  isMilestoneEvent,
  isOverlayEvent,
  isPinboardEvent,
  MILESTONE_CALENDAR_ID,
  occurrenceKey,
  PINBOARD_CALENDAR_ID,
  startOfDay,
  startOfWeek,
  type DraftState,
  type LandedGhost,
} from "../../lib/calendar-layout";
import { pinboardEntries, type PinboardEntry } from "../../lib/pinboard/calendarEntries";
import { PINBOARD_PREF_KEY } from "../../lib/pinboard/types";
import {
  deviceTimeZoneOrNull,
  milestoneColor,
  pinboardColor,
  sourceColors,
  sourceShapeIndex,
  useTheme,
  useUserTime,
} from "../../theme";
import { Callout, Skeleton } from "../ui";
import { useNowTick } from "../../lib/useNowTick";
import { CalendarEventPopover, type PopoverEditing } from "./parts/CalendarEventPopover";
import { whenText } from "./parts/whenText";
import { CalendarHeader } from "./CalendarHeader";
import { DeleteEventDialog } from "./edit/DeleteEventDialog";
import { EventEditor } from "./edit/EventEditor";
import { NewEventEditor, type CreateHandover } from "./edit/NewEventEditor";
import { QuickCreatePopover, type LeftDraft } from "./edit/QuickCreatePopover";
import { WriteNotices } from "./edit/WriteNotices";
import { undoLatest, useDeleteDialog, useEventWrites, useMirrorReads } from "./edit/useEventWrites";
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
// A quick-create draft whose box went with the tab, titled and with nothing sent (a click in the
// sidebar switches tabs, and a titled box stays through a click): it opens again on return.
let leftQuick: LeftDraft | null = null;
const leaveQuick = (left: LeftDraft) => {
  leftQuick = left;
};

/** How many days the grid shows for a view. Week is 7 by definition; Day is the user's chosen
 *  width (1-6). Anything else isn't a day grid. */
function windowDays(view: CalendarViewMode, dayCount: number): number {
  if (view === "week") return 7;
  if (view === "day") return dayCount;
  return 0;
}

/** Step the cursor by one period in the current view's units. The day grids move by a WHOLE
 *  window — 7 for Week, N for an N-day Day view — so the arrows page rather than nudge; the
 *  sideways swipe moves as many days as Settings chooses, one unless changed (`swipeDays`). */
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
  /** Open a project's page — wired to the milestone overlay events, and to a synced event's linked
   *  milestone in its details. */
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
  const now = useNowTick();

  useEffect(() => {
    aliveRef.current = true;
    return () => {
      aliveRef.current = false;
    };
  }, []);

  // A quick create that saved: its ghost (the new event as it was typed) stays until a read of the
  // mirror that started after the save has landed, so the event doesn't blink out between the box
  // closing and its row arriving (`ghostAfterRead`).
  const [landed, setLanded] = useState<LandedGhost | null>(null);
  const readsStarted = useRef(0);
  const readEvents = useCallback(() => {
    readsStarted.current += 1;
    return listAllCalendarEvents();
  }, []);
  const applyEvents = useCallback((evts: CalendarEvent[]) => {
    setEvents(evts);
    cachedEvents = evts;
    setEventsFailed(false);
    // Only the newest read started is applied, so this is it.
    const started = readsStarted.current;
    setLanded((l) => ghostAfterRead(l, started));
  }, []);
  const eventsReadFailed = useCallback(() => {
    // Keep the last-good events; a read failure is transient. Flag it only when we have nothing to
    // show, so the empty grid isn't mistaken for "no events".
    if (cachedEvents.length === 0) setEventsFailed(true);
  }, []);
  // Only the newest read applies (an older in-flight one, e.g. a focus refresh, can't overwrite a
  // newer one that resolved first), and each is marked, so rows a delete hid are shown again only
  // by a read that knows how it ended. A delete that settles reads again.
  const loadEvents = useMirrorReads(readEvents, applyEvents, eventsReadFailed);

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

  // A save or delete (#884) rewrote part of the mirror: re-read the events. The event comes from the
  // backend, so a delete that settles after a tab switch still refreshes the view it lands on.
  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | null = null;
    // Milestones too: a linked one takes its date from its event, so moving the event moves it.
    void onCalendarWriteLanded(() => {
      void loadEvents();
      void loadMilestones();
    })
      .then((stop) => {
        if (alive) unlisten = stop;
        else stop();
      })
      .catch(() => {});
    return () => {
      alive = false;
      unlisten?.();
    };
  }, [loadEvents, loadMilestones]);

  // A sync or the ~30 s change check wrote events (a change made in Google, say): re-read, so the
  // open calendar shows it without waiting for a focus change. The overview too, for "synced".
  useEffect(() => {
    let alive = true;
    let unlisten: (() => void) | null = null;
    void onCalendarSynced(() => {
      void loadEvents();
      void loadOverview();
      void loadMilestones();
    })
      .then((stop) => {
        if (alive) unlisten = stop;
        else stop();
      })
      .catch(() => {});
    return () => {
      alive = false;
      unlisten?.();
    };
  }, [loadEvents, loadOverview, loadMilestones]);

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

  // Whether the event popover is open: it isn't a Modal, so the dialog registry can't see it, and ←
  // paging the grid under an open popover leaves it pointing at an event no longer on screen.
  const popoverOpenRef = useRef(false);
  // The event popup that's open, anchored at the clicked element's rect (null = closed).
  const [eventPopup, setEventPopup] = useState<{ ev: CalendarEvent; anchor: DOMRect } | null>(null);
  useEffect(() => {
    popoverOpenRef.current = eventPopup !== null;
  }, [eventPopup]);
  const popoverOpen = useCallback(() => popoverOpenRef.current, []);
  // The popover's own close, which hands focus back as its Escape does (it fills this in).
  const popoverCloseRef = useRef<(() => void) | null>(null);

  // A new event being made (#884, C10): its starting fields and calendar, and where: the full editor,
  // or quick create under "+ New event" (`anchor`, the button's rect), which More options turns into
  // the full editor carrying its draft (`handover`), and which opens again on a draft that went with
  // the tab (`resume`). Null = closed.
  const [creating, setCreating] = useState<{
    seed: EditorFields;
    calendarId: string;
    mode: "quick" | "full";
    anchor: DOMRect | null;
    handover?: CreateHandover;
    resume?: LeftDraft;
  } | null>(null);
  // The open surface's way to bring its title into focus: "+ New event" again, or a click on the
  // ghost, focuses it, never replaces it.
  const focusCreateRef = useRef<(() => void) | null>(null);
  // "+ New event": quick create opens under it, and focus goes back to it when that closes.
  const createButtonRef = useRef<HTMLButtonElement>(null);
  // Quick create's draft as it's typed, and how far it has got, for the ghost on the grid. The ref is
  // for its close, which turns the last of it into the ghost that waits for the saved row.
  const [quickDraft, setQuickDraft] = useState<{
    draft: EditorFields;
    calendarId: string;
    state: DraftState;
  } | null>(null);
  const quickDraftRef = useRef(quickDraft);
  const onQuickDraft = useCallback((draft: EditorFields, calendarId: string, state: DraftState) => {
    quickDraftRef.current = { draft, calendarId, state };
    setQuickDraft(quickDraftRef.current);
  }, []);
  // Where a new event can go. With no Google account there's no button at all; with one but no
  // calendar to take an event, the button says why.
  const targets = useMemo(() => (overview ? createTargets(overview) : null), [overview]);
  const hasGoogle = overview?.accounts.some((a) => a.provider === "google") ?? false;
  const createReason = overview && targets ? createReasonFor(overview, targets) : null;
  // "+ New event" and `c` open the full editor; Shift+C quick create under the button (as Google's
  // keys do; without the button on screen, the full editor). Either starts on the calendar last
  // created in (else the primary), at the next half hour today when today is on screen, else at
  // 09:00 on the day the view is on, for the account's own default length. Times are this
  // computer's; without its zone, the calendar's (and then nothing can be saved, which they say).
  const startCreate = useCallback(
    (mode: "quick" | "full") => {
      if (creating) {
        focusCreateRef.current?.();
        return;
      }
      if (!overview || !targets) return;
      const target = defaultTarget(targets, readLastCreate());
      if (!target) return;
      const anchor =
        mode === "quick" ? (createButtonRef.current?.getBoundingClientRect() ?? null) : null;
      const now = new Date();
      const { start, end } = visibleRange(view, cursor, dayCount);
      const today = startOfDay(now).getTime();
      const day = today >= start.getTime() && today < end.getTime() ? now : cursor;
      const zone = deviceTimeZoneOrNull() ?? target.calendar.time_zone ?? null;
      // A saved event's ghost still waiting for its row gives way to the new one.
      setLanded(null);
      setCreating({
        seed: newEventSeed(dayKey(startOfDay(day)), now, zone, lengthFor(overview, target)),
        calendarId: target.calendar.id,
        mode: anchor ? mode : "full",
        anchor,
      });
    },
    [creating, overview, targets, view, cursor, dayCount],
  );
  const openCreate = useCallback(() => startCreate("full"), [startCreate]);

  // Quick create closed: when it saved, its last draft stays drawn until a read that started after
  // the save lands (one starts now: the read the save itself set off may have begun before it).
  const onQuickClose = useCallback(
    (saved: boolean) => {
      const last = quickDraftRef.current;
      const ev = saved && last ? draftEvent(last.draft, last.calendarId, "saved") : null;
      if (ev) {
        setLanded({ ev, after: readsStarted.current });
        void loadEvents();
      }
      quickDraftRef.current = null;
      setQuickDraft(null);
      setCreating(null);
    },
    [loadEvents],
  );
  // More options: the full editor carries on with quick create's draft; nothing is sent.
  const onQuickMore = useCallback((handover: CreateHandover) => {
    quickDraftRef.current = null;
    setQuickDraft(null);
    setCreating((c) => (c ? { ...c, mode: "full", anchor: null, handover } : c));
  }, []);

  // Back on the calendar after a titled quick create went with the tab (`leftQuick`): the box opens
  // again on that draft, on its calendar while that can still take an event (else the usual one). It
  // opens with what's known at once (the overview kept from before), as the box did; a calendar on
  // offer is a Google account's, so "+ New event" is there to open under.
  useEffect(() => {
    if (!overview || !targets || leftQuick === null) return;
    const left = leftQuick;
    leftQuick = null;
    const target = defaultTarget(targets, left.calendarId);
    const anchor = createButtonRef.current?.getBoundingClientRect();
    if (!target || !anchor) return;
    setCreating({
      seed: left.seed,
      calendarId: target.calendar.id,
      mode: "quick",
      anchor,
      resume: left,
    });
  }, [overview, targets]);

  // One step per swipe, in the direction of travel: a swipe left (positive deltaX) moves forward, by
  // the days Settings chooses (`swipeDays`, read at the swipe). Both gestures move the cursor and
  // nothing else — `writeCursorDay` above is what remembers it, and `openOn` decides whether that
  // memory is ever read back. Not under an open popover, for the reason ← and → aren't (above), nor
  // under a new event's editor.
  const gridRef = useHorizontalWheelShift(
    (dir) => setCursor((c) => addDays(c, dir * swipeDays(windowDays(view, dayCount)))),
    (view === "day" || view === "week") && eventPopup === null && creating === null,
  );

  // Keyboard nav while the tab is mounted: ← / → step the period, `t` jumps to today, `c` starts a
  // new event in the full editor and Shift+C in quick create. Ignored while a field is focused, a
  // modifier is held (so app shortcuts and text entry are untouched), or a dialog (quick create
  // included) or the event popover is open over the tab (see `calendarNavKey`). Ctrl/Cmd+Z is the
  // delete's Undo (`calendarUndoKey`).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      // Ctrl/Cmd+Z presses the newest Undo on screen (a delete still waiting). Unlike the arrows it
      // works with an event's details open (the pop-up has no fields), and closes them: the event
      // coming back reflows the grid, and the pop-up stays pinned where it was opened. A press with
      // nothing to undo is left alone.
      if (calendarUndoKey(e, isAnyDialogOpen())) {
        if (undoLatest()) {
          e.preventDefault();
          if (popoverOpenRef.current) (popoverCloseRef.current ?? (() => setEventPopup(null)))();
        }
        return;
      }
      const nav = calendarNavKey(e, isAnyDialogOpen() || popoverOpenRef.current);
      if (nav === "prev") {
        e.preventDefault();
        onPrev();
      } else if (nav === "next") {
        e.preventDefault();
        onNext();
      } else if (nav === "today") {
        onToday();
      } else if (nav === "create") {
        e.preventDefault();
        startCreate("full");
      } else if (nav === "quick-create") {
        e.preventDefault();
        startCreate("quick");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onPrev, onNext, onToday, startCreate]);

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

  const writes = useEventWrites();

  // What PM may do to a synced event (#884): the row's own block, then its account's editing status.
  // The list computes `edit_block` assuming editing is on; the overview says whether it is.
  const editingFor = useCallback(
    (ev: CalendarEvent): { canEdit: boolean; canDelete: boolean; reason: string | null } => {
      const none = (reason: string | null) => ({ canEdit: false, canDelete: false, reason });
      const cal = overview?.calendars.find((c) => c.id === ev.calendar_id);
      if (ev.edit_block) return none(reasonText(ev.edit_block));
      if (cal?.provider !== "google") return none(null);
      if (overview?.editing[cal.source_id] !== "on") return none(reasonText("editing_off"));
      // A locked copy keeps its busy/free and visibility editable, but Google's delete of one isn't
      // documented, so PM doesn't offer it (the gate agrees on save).
      return {
        canEdit: true,
        canDelete: !ev.locked,
        reason: ev.locked ? reasonText("locked") : null,
      };
    },
    [overview],
  );

  const visibleEvents = useMemo(() => {
    // Filter to visible calendars, THEN dedup the same physical OCCURRENCE mirrored on two of them
    // (same iCal UID *and* start), keeping one visible copy — otherwise it renders twice in the
    // grid/agenda. Done after the hide filter so hiding one calendar still shows the copy on the
    // calendar left visible. Keyed on the UID alone this also collapsed every recurring series to one
    // occurrence across the whole mirror — see `occurrenceKey`, which FocusUpcoming shares. The copy
    // kept is the first, unless a later one can be changed and it can't: an event you organise on
    // your own calendar beats the invitation copy of it on a shared one.
    //
    // A row a delete is hiding takes every copy of its occurrence with it: the same Google event
    // seen through a second connected account is the same event, and showing that copy instead
    // would look like the delete hadn't happened.
    const deleting = new Set(
      events
        .filter((e) => writes.hiddenIds.has(e.id))
        .map(occurrenceKey)
        .filter((k): k is string => k !== null),
    );
    const at = new Map<string, number>();
    const out: CalendarEvent[] = [];
    for (const e of events) {
      if (hidden.has(e.calendar_id) || writes.hiddenIds.has(e.id)) continue;
      const key = occurrenceKey(e);
      if (key !== null && deleting.has(key)) continue;
      if (key !== null) {
        const i = at.get(key);
        if (i !== undefined) {
          if (!editingFor(out[i]).canDelete && editingFor(e).canDelete) out[i] = e;
          continue;
        }
        at.set(key, out.length);
      }
      out.push(e);
    }
    // Both first-party overlays ride on top, each toggleable like a calendar via the same `hidden` set.
    // Their events carry a null uid + a unique calendar_id, so they skip the dedup and hide filter
    // cleanly.
    if (!hidden.has(MILESTONE_CALENDAR_ID)) out.push(...milestoneEvents);
    if (!hidden.has(PINBOARD_CALENDAR_ID)) out.push(...pinboardEvents);
    return out;
  }, [events, hidden, milestoneEvents, pinboardEvents, writes.hiddenIds, editingFor]);

  // The new event's ghost (C10c): quick create's draft in its calendar's colour, saying how far it
  // has got, or a saved one's waiting for its row. It joins after the hide filter, so it shows on a
  // hidden calendar too (the box says that calendar is hidden), and in its own memo, so a title
  // keystroke doesn't run the filter and dedup above again. Not while the full editor is open: that
  // covers the grid.
  const ghost = useMemo(() => {
    if (creating?.mode === "quick") {
      return quickDraft
        ? draftEvent(quickDraft.draft, quickDraft.calendarId, quickDraft.state)
        : null;
    }
    return landed?.ev ?? null;
  }, [creating?.mode, quickDraft, landed]);
  const shownEvents = useMemo(
    () => (ghost ? [...visibleEvents, ghost] : visibleEvents),
    [visibleEvents, ghost],
  );

  // The popover's editing actions for `ev`. Delete closes the popover and asks first.
  // The event open in the editor (null = closed).
  const [editing, setEditing] = useState<CalendarEvent | null>(null);
  // The delete dialog: the event it asks about (null = closed), and the delete it sends. It sees open
  // dialogs (the editor among them) itself; the popover isn't one.
  const deleteDialog = useDeleteDialog(popoverOpen);
  const confirmDelete = deleteDialog.asked?.event ?? null;
  // People already in the user's Google and Outlook calendars, for the editors' guest box (an event's
  // and a new one's): worked out here, over the rows this view holds, so no contacts are read (C12;
  // never a subscription's).
  const suggestions = useMemo(
    () => guestSuggestions(events, overview?.calendars ?? [], null),
    [events, overview],
  );
  // The address of the account a synced event's calendar belongs to.
  const accountOf = (ev: CalendarEvent): string | null => {
    const cal = overview?.calendars.find((c) => c.id === ev.calendar_id);
    return overview?.accounts.find((a) => a.id === cal?.source_id)?.email ?? null;
  };
  const popoverEditing = (ev: CalendarEvent): PopoverEditing => ({
    ...editingFor(ev),
    onEdit: () => {
      setEventPopup(null);
      setEditing(ev);
    },
    onDelete: () => {
      setEventPopup(null);
      deleteDialog.ask(ev);
    },
  });

  // Clicking an event: a PM overlay keeps its jump (a milestone opens its project, a freeform pinboard
  // entry opens the Pinboard), while a real synced event opens the in-place detail popup anchored at
  // the click. Every event is now clickable (was overlay-only). While quick create is open, a click
  // on any event (its own ghost included) goes back to it: an overlay's jump would switch tabs and
  // lose the draft, and an event's details would be a second box with a second primary button.
  const quickOpen = creating?.mode === "quick";
  const onEventClick = useCallback(
    (ev: CalendarEvent, anchor: DOMRect) => {
      if (isDraftEvent(ev) || quickOpen) {
        focusCreateRef.current?.();
        return;
      }
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
    [quickOpen, milestones, onOpenProject, onOpenPinboard],
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
    // The ghost of a new event joins the pixel views and the Agenda (`shownEvents`); the Year
    // overview and the Terminal tables show what's in the calendar only.
    switch (view) {
      case "agenda":
        return (
          <AgendaView
            events={shownEvents}
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
            events={shownEvents}
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
            events={shownEvents}
            colorOf={colorOf}
            range={range}
            bounds={activeBounds}
            customBounds={customBounds[range]}
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
        onCreate={hasGoogle ? openCreate : undefined}
        createReason={createReason}
        createRef={createButtonRef}
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
            here.
          </p>
        </div>
      ) : (
        // The swipe target. `ref={gridRef}` was missing entirely until now, so the Calendar tab's
        // horizontal swipe had never once fired despite the v3.88.0 notes advertising it here
        // alongside Focus's. Note WHERE this branch sits: it renders only once `overview` has
        // resolved, and the day columns inside it are keyed by date, so a step unmounts the one
        // leaving the window. `useHorizontalWheelShift` owns neither problem from here — it listens on the
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
          closeRef={popoverCloseRef}
          onOpenProject={
            onOpenProject
              ? (p) => {
                  setEventPopup(null);
                  onOpenProject(p);
                }
              : undefined
          }
          editing={popoverEditing(eventPopup.ev)}
          describeSeries={writes.describeSeries}
        />
      )}

      {/* Dialogs and notices render here, beside the popover and never inside the animated body
          above: its fade-up uses `transform`, which would reposition anything fixed inside it. */}
      {editing && (
        <EventEditor
          key={editing.id}
          row={editing}
          calendar={overview?.calendars.find((c) => c.id === editing.calendar_id) ?? null}
          account={accountOf(editing)}
          suggestions={suggestions}
          milestone={
            editing.uid
              ? (milestones.find((m) => m.event_uid && m.event_uid === editing.uid) ?? null)
              : null
          }
          onDelete={(row) => {
            // Close the editor first so its focus goes back to the event's chip, and the delete
            // dialog that opens next returns it there too.
            flushSync(() => setEditing(null));
            deleteDialog.ask(row);
          }}
          onClose={() => setEditing(null)}
        />
      )}
      {creating?.mode === "full" && overview && targets && (
        <NewEventEditor
          seed={creating.seed}
          targets={targets}
          calendarId={creating.calendarId}
          overview={overview}
          hidden={hidden}
          suggestions={suggestions}
          colorOf={colorOf}
          shapeOf={shapeOf}
          focusRef={focusCreateRef}
          handover={creating.handover}
          onClose={() => {
            // From quick create, the editor opened from its More options, which is gone: focus
            // goes back to "+ New event", as quick create's own close would have sent it.
            const fromQuick = creating.handover !== undefined;
            setCreating(null);
            if (fromQuick) createButtonRef.current?.focus();
          }}
        />
      )}
      {creating?.mode === "quick" && creating.anchor && overview && targets && (
        <QuickCreatePopover
          anchor={creating.anchor}
          seed={creating.seed}
          targets={targets}
          calendarId={creating.calendarId}
          overview={overview}
          hidden={hidden}
          colorOf={colorOf}
          shapeOf={shapeOf}
          returnFocusTo={createButtonRef}
          focusRef={focusCreateRef}
          resume={creating.resume}
          onDraft={onQuickDraft}
          onMore={onQuickMore}
          onClose={onQuickClose}
          onLeave={leaveQuick}
        />
      )}
      <DeleteEventDialog
        event={confirmDelete}
        calendarName={
          overview?.calendars.find((c) => c.id === confirmDelete?.calendar_id)?.name ?? null
        }
        when={confirmDelete ? whenText(confirmDelete) : ""}
        from={confirmDelete ? accountOf(confirmDelete) : null}
        again={deleteDialog.asked?.reask?.again ?? false}
        forceRepeating={!!deleteDialog.asked?.reask}
        guests={deleteDialog.asked?.guests ?? null}
        onClose={deleteDialog.close}
        onConfirm={deleteDialog.confirm}
      />
      <WriteNotices
        notices={writes.notices}
        onUndo={(token) => void writes.undoDelete(token)}
        onDismiss={writes.dismissNotice}
        onSettle={(token, action, title) => void writes.settleSplit(token, action, title)}
        onCheckCreate={(id) =>
          void writes.checkCreate(id, (calendarId) =>
            overview
              ? createdWhere(overview, hidden, calendarId)
              : { name: "that calendar", hidden: false },
          )
        }
        // A move writes no calendar row, so no write-outcome follows: the milestones are read
        // again here, for the popover's "Open in Project" and the editor's line to follow it.
        onMove={(id) =>
          void writes.moveMilestone(id).then((moved) => {
            if (moved) void loadMilestones();
          })
        }
        onMoveBack={(id) =>
          void writes.moveMilestoneBack(id).then((moved) => {
            if (moved) void loadMilestones();
          })
        }
        onHold={writes.holdMoveBack}
      />
    </div>
  );
}
