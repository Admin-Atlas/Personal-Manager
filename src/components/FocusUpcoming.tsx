// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Focus tab's "Upcoming" card. Two display modes, chosen from the header toggle:
//   • "list"  — the plain agenda: the next handful of events, one per row (the long-standing default).
//   • "week"  — a compact few-day time grid, reusing the exact Calendar Week engine (TimeGridView),
//               capped to 1–4 days so it fits the Focus column at the same width. ‹ / › step the
//               window one day at a time; the window starts at today each time the tab opens and holds
//               where you leave it while you're there (a "Today" chip snaps back). Work / Day frame
//               the visible hours, and the day count sits beside them.
//
// Every one of those controls lives HERE, beside what it changes; Settings no longer mirrors them.
//
// Three things this card does NOT share with the Calendar tab, all because it is a ~26rem pane rather
// than a full page: it offers no 24h range (at this height a whole day's rows can't hold a legible
// event card — the grid still scrolls the full 24h, so nothing is out of reach); it hands
// TimeGridView a much lower row-height floor, so the range it IS showing fills the pane instead of
// bottoming out on the calendar's floor and rendering every wide window identically; and its Work/Day
// hour windows are its OWN (readFocusUpcomingBounds), so tightening Work to suit this pane doesn't
// tighten the full-page week grid. The editor itself is the Calendar tab's RangeControl, reused.
//
// The list uses the focus agenda feed the parent already loads; the grid lazily pulls the full mirror
// (listAllCalendarEvents) so days either side of today are populated. Synced events only — the same
// set the agenda shows — with no first-party overlays (milestones live on the Calendar tab).

import { useEffect, useMemo, useState } from "react";
import type { AgendaEvent, CalendarEvent } from "../lib/types";
import { listAllCalendarEvents } from "../lib/ipc";
import { resolveRangeBounds } from "../lib/calendarGeom";
import type { CalendarRange, RangeBounds } from "../lib/calendarPrefs";
import { addDays, dayKey, startOfDay } from "../lib/calendar-layout";
import { sourceColors, useTheme, useUserTime } from "../theme";
import { formatEventWhen } from "../lib/format";
import { Card, SegmentedControl } from "./ui";
import { TimeGridView } from "./calendar/views/TimeGridView";
import { RangeControl } from "./calendar/RangeControl";
import {
  FOCUS_UPCOMING_DAY_CHOICES,
  FOCUS_UPCOMING_RANGES,
  clampFocusUpcomingDays,
  readFocusUpcomingBounds,
  readFocusUpcomingDays,
  readFocusUpcomingMode,
  readFocusUpcomingRange,
  writeFocusUpcomingBounds,
  writeFocusUpcomingDays,
  writeFocusUpcomingMode,
  writeFocusUpcomingRange,
  type FocusUpcomingMode,
} from "../lib/focusPrefs";

const NOOP = () => {};

/** Row-height floor for this pane — well under the calendar's 20px, so the chosen window fills the
 *  card whole rather than hitting the floor and scrolling. See the header comment. */
const COMPACT_MIN_ROW_H = 12;

// Last-good full mirror for the grid, kept in module scope so switching to Week (or back to the tab)
// doesn't flash an empty grid before the read lands — mirrors FocusView's other caches.
let cachedAllEvents: CalendarEvent[] = [];

interface Props {
  /** The focus agenda feed (upcoming synced events, incl. today's already-ended). Used by List mode. */
  listEvents: AgendaEvent[];
  /** Ids of the connected calendars, for colouring the grid's events. */
  calendarIds: string[];
}

export function FocusUpcoming({ listEvents, calendarIds }: Props) {
  const { system, accent, colorblind } = useTheme();
  const { coords } = useUserTime();
  const [mode, setMode] = useState<FocusUpcomingMode>(readFocusUpcomingMode);
  const [range, setRange] = useState<CalendarRange>(readFocusUpcomingRange);
  // This pane's own Work/Day hour windows (empty ⇒ the computed defaults). Edited from the ▾ on the
  // Work/Day control, exactly as on the Calendar tab — but stored separately, see the header comment.
  const [bounds, setBounds] =
    useState<Partial<Record<CalendarRange, RangeBounds>>>(readFocusUpcomingBounds);
  // 1–4, from the header control below; read once on mount like the others.
  const [days, setDays] = useState<number>(() => clampFocusUpcomingDays(readFocusUpcomingDays()));
  // The leftmost visible day. Starts at today each open; ‹ / › move it, "Today" snaps back. Kept as
  // component state (not persisted) so the window never jumps under you mid-session, and "Upcoming"
  // always opens on today.
  const [anchor, setAnchor] = useState<Date>(() => startOfDay(new Date()));
  const [allEvents, setAllEvents] = useState<CalendarEvent[]>(() => cachedAllEvents);

  function changeMode(next: FocusUpcomingMode) {
    setMode(next);
    writeFocusUpcomingMode(next);
  }
  function changeRange(next: CalendarRange) {
    setRange(next);
    writeFocusUpcomingRange(next);
  }
  function changeDays(next: number) {
    const clamped = clampFocusUpcomingDays(next);
    setDays(clamped);
    writeFocusUpcomingDays(clamped);
  }
  /** Set (or clear, with `null`) one range's custom window. */
  function changeBounds(which: CalendarRange, next: RangeBounds | null) {
    setBounds((prev) => {
      const map = { ...prev };
      if (next) map[which] = next;
      else delete map[which];
      writeFocusUpcomingBounds(map);
      return map;
    });
  }

  // Lazily load the full mirror only while the grid is on (List mode needs nothing extra). Refresh on
  // window focus so an edit made elsewhere shows on return. Keep the last-good set on a read failure.
  useEffect(() => {
    if (mode !== "week") return;
    let alive = true;
    const load = () => {
      void listAllCalendarEvents()
        .then((evts) => {
          if (!alive) return;
          setAllEvents(evts);
          cachedAllEvents = evts;
        })
        .catch(() => {
          /* keep the last-good events */
        });
    };
    load();
    window.addEventListener("focus", load);
    return () => {
      alive = false;
      window.removeEventListener("focus", load);
    };
  }, [mode]);

  const gridDays = useMemo(
    () => Array.from({ length: days }, (_, i) => addDays(anchor, i)),
    [anchor, days],
  );

  // Dedup the same physical event mirrored on two calendars (same iCal UID), keeping the first — the
  // grid buckets by day itself, so events outside the window simply aren't placed.
  const gridEvents = useMemo(() => {
    const seen = new Set<string>();
    const out: CalendarEvent[] = [];
    for (const e of allEvents) {
      if (e.uid) {
        if (seen.has(e.uid)) continue;
        seen.add(e.uid);
      }
      out.push(e);
    }
    return out;
  }, [allEvents]);

  const colorOf = useMemo(() => {
    const map = sourceColors(calendarIds, system, accent, colorblind);
    return (calendarId: string) => map.get(calendarId) ?? "var(--ink4)";
  }, [calendarIds, system, accent, colorblind]);

  const visibleBounds = useMemo(
    () => resolveRangeBounds(range, bounds, coords, anchor),
    [range, bounds, coords, anchor],
  );

  const anchoredToday = dayKey(anchor) === dayKey(startOfDay(new Date()));
  const rangeLabel = useMemo(() => {
    const fmt = (d: Date) => d.toLocaleDateString(undefined, { weekday: "short", day: "numeric" });
    const first = gridDays[0];
    const last = gridDays[gridDays.length - 1];
    return days === 1 ? fmt(first) : `${fmt(first)} – ${fmt(last)}`;
  }, [gridDays, days]);

  const navBtn =
    "rounded-[var(--radius-sm)] px-1.5 py-0.5 text-ink3 transition hover:bg-surface hover:text-ink2";

  return (
    <Card className="mb-5 px-4 py-3" data-help="focus-agenda">
      <div className="mb-2 flex flex-wrap items-center justify-between gap-2">
        <h2 className="font-mono text-xs font-semibold uppercase tracking-wide text-ink3">
          Upcoming
        </h2>
        <SegmentedControl
          value={mode}
          onChange={changeMode}
          options={[
            { value: "list", label: "List", title: "A simple agenda list" },
            { value: "week", label: "Days", title: "A day-by-day calendar grid" },
          ]}
        />
      </div>

      {mode === "list" ? (
        <AgendaList events={listEvents} />
      ) : (
        <>
          <div className="mb-2 flex flex-wrap items-center justify-between gap-2 text-xs text-ink3">
            <div className="flex items-center gap-1">
              <button
                type="button"
                onClick={() => setAnchor((a) => addDays(a, -1))}
                title="Previous day"
                className={navBtn}
              >
                ‹
              </button>
              <span className="tabular-nums">{rangeLabel}</span>
              <button
                type="button"
                onClick={() => setAnchor((a) => addDays(a, 1))}
                title="Next day"
                className={navBtn}
              >
                ›
              </button>
              {!anchoredToday && (
                <button
                  type="button"
                  onClick={() => setAnchor(startOfDay(new Date()))}
                  title="Back to today"
                  className={`${navBtn} ml-1`}
                >
                  Today
                </button>
              )}
            </div>
            <div className="flex items-center gap-1.5">
              <SegmentedControl
                value={String(days)}
                onChange={(v) => changeDays(Number(v))}
                options={FOCUS_UPCOMING_DAY_CHOICES.map((n) => ({
                  value: String(n),
                  label: String(n),
                  title: n === 1 ? "One day" : `${n} days`,
                }))}
              />
              {/* The Calendar tab's control, narrowed to Work/Day: each carries a ▾ that sets the
                  hours it frames. */}
              <RangeControl
                range={range}
                onRangeChange={changeRange}
                customBounds={bounds}
                onBoundsChange={changeBounds}
                coords={coords}
                cursor={anchor}
                ranges={FOCUS_UPCOMING_RANGES}
              />
            </div>
          </div>
          <div className="h-[26rem]">
            <TimeGridView
              days={gridDays}
              events={gridEvents}
              colorOf={colorOf}
              range={range}
              bounds={visibleBounds}
              zones={[]}
              onZonesChange={NOOP}
              allowZones={false}
              minRowHeight={COMPACT_MIN_ROW_H}
            />
          </div>
        </>
      )}
    </Card>
  );
}

/** The agenda list — the next handful of events, one per row. An event that already ended today stays
 *  listed (a real day stays visible until its own midnight) but greyed.
 *
 *  The name WRAPS rather than truncating. This card is ~22rem wide next to a 8rem time column, so
 *  "…" swallowed most real meeting titles — and a title you can't read is the one thing the row is
 *  for. The time stays on one line as a fixed gutter, and the name/location column wraps under it. */
function AgendaList({ events }: { events: AgendaEvent[] }) {
  const shown = events.slice(0, 8);
  return (
    <>
      <ul className="flex flex-col gap-1.5">
        {shown.map((e) => (
          <li key={e.id} className={`flex gap-3 text-sm${e.ended ? " opacity-45" : ""}`}>
            <span className="w-32 shrink-0 font-mono text-xs leading-5 text-ink3">
              {formatEventWhen(e.start, e.all_day)}
            </span>
            <span className="min-w-0 flex-1">
              {/* `break-words` so a single unbroken token (a long URL-ish title) still folds rather
                  than forcing the card to scroll sideways. */}
              <span className="break-words text-ink2">{e.summary}</span>
              {e.location && (
                <span className="ml-2 break-words text-xs text-ink4">{e.location}</span>
              )}
              {e.ended && <span className="ml-2 whitespace-nowrap text-xs text-ink4">ended</span>}
            </span>
          </li>
        ))}
      </ul>
      {events.length > shown.length && (
        <p className="mt-2 text-xs text-ink4">+{events.length - shown.length} more</p>
      )}
    </>
  );
}
