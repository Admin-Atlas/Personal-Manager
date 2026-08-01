// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The calendar's shared chrome — one header, no second sidebar. View switcher (active = accent),
// ‹ Today › nav, the date-range label, the Calendars dropdown, and Refresh-now.
//
// The last-synced time rides ON the Refresh button rather than beside it: it is the one fact that
// tells you whether pressing Refresh is worth it. The old separate "Read-only" badge is gone — it was
// a static, prop-free span nothing derived from, and the calendar has no edit affordances to
// disambiguate; the fact is still stated in the help card, the empty state and the Connectors copy.

import {
  DAY_COUNT_MAX,
  DAY_COUNT_MIN,
  type CalendarRange,
  type CalendarViewMode,
  type RangeBounds,
} from "../../lib/calendarPrefs";
import { formatSyncedShort, formatWhen } from "../../lib/format";
import { useDepth, useTheme, type Coords } from "../../theme";
import { Button, SegmentedControl, Select, type SegOption } from "../ui";
import { MiniCalendarPopover } from "./MiniCalendarPopover";
import { RangeControl } from "./RangeControl";

interface Props {
  view: CalendarViewMode;
  availableViews: readonly CalendarViewMode[];
  onViewChange: (v: CalendarViewMode) => void;
  /** Time-grid vertical scale (Week/Day only). */
  range: CalendarRange;
  onRangeChange: (r: CalendarRange) => void;
  /** How many days the Day view shows (1-6). Week is fixed at 7 and hides this control. */
  dayCount: number;
  onDayCountChange: (n: number) => void;
  /** Custom Work/Day hour windows + the setter (null clears back to the computed default). */
  customBounds: Partial<Record<CalendarRange, RangeBounds>>;
  onBoundsChange: (range: CalendarRange, bounds: RangeBounds | null) => void;
  /** The user's coordinates, for the Day range's sunrise/sunset default. */
  coords: Coords | null;
  /** The current period label (e.g. "July 2026") shown between the nav arrows. */
  label: string;
  /** The anchor day, for the mini-calendar picker. */
  cursor: Date;
  onPickDate: (d: Date) => void;
  onPrev: () => void;
  onNext: () => void;
  onToday: () => void;
  onRefresh: () => void;
  syncing: boolean;
  lastSync: string | null;
}

const VIEW_LABEL: Record<CalendarViewMode, string> = {
  month: "Month",
  week: "Week",
  day: "Day",
  year: "Year",
  agenda: "Agenda",
};

export function CalendarHeader({
  view,
  availableViews,
  onViewChange,
  range,
  onRangeChange,
  dayCount,
  onDayCountChange,
  customBounds,
  onBoundsChange,
  coords,
  label,
  cursor,
  onPickDate,
  onPrev,
  onNext,
  onToday,
  onRefresh,
  syncing,
  lastSync,
}: Props) {
  const { showMeta } = useDepth();
  const { system } = useTheme();
  // The last successful sync: clock time if it was today, else the date. Null when unset/unparseable.
  const synced = lastSync ? formatSyncedShort(lastSync) || null : null;
  // The Work/Day/24h scale only drives the pixel time-grid (Slate/Editorial). Terminal renders
  // Week/Day as a mono agenda with no vertical scale, so the control has nothing to act on there.
  const showRange = (view === "week" || view === "day") && system !== "terminal";

  const viewOptions: SegOption<CalendarViewMode>[] = availableViews.map((v) => ({
    value: v,
    label: VIEW_LABEL[v],
  }));

  return (
    <header
      className="flex flex-wrap items-center gap-x-4 gap-y-2 border-b border-rule px-4 py-2"
      data-help="calendar-header"
    >
      <div className="flex items-center gap-1">
        <Button variant="tertiary" onClick={onPrev} title="Previous" aria-label="Previous">
          ‹
        </Button>
        <Button variant="tertiary" onClick={onToday} title="Jump to today">
          Today
        </Button>
        <Button variant="tertiary" onClick={onNext} title="Next" aria-label="Next">
          ›
        </Button>
      </div>

      <MiniCalendarPopover label={label} cursor={cursor} onPick={onPickDate} />

      <div className="ml-auto flex flex-wrap items-center gap-3">
        {view === "day" && (
          // Day only: Week IS the 7-day window, so offering a width control there would be two
          // controls producing the same picture. Sits with the range control because both answer
          // "how much am I looking at", one across and one down.
          <label className="flex items-center gap-1.5 text-xs text-ink3">
            Days
            <Select
              compact
              value={String(dayCount)}
              onChange={(e) => onDayCountChange(Number(e.currentTarget.value))}
              aria-label="How many days to show"
            >
              {Array.from({ length: DAY_COUNT_MAX - DAY_COUNT_MIN + 1 }, (_, i) => {
                const n = DAY_COUNT_MIN + i;
                return (
                  <option key={n} value={n}>
                    {n}
                  </option>
                );
              })}
            </Select>
          </label>
        )}
        {showRange && (
          <>
            <RangeControl
              range={range}
              onRangeChange={onRangeChange}
              customBounds={customBounds}
              onBoundsChange={onBoundsChange}
              coords={coords}
              cursor={cursor}
            />
          </>
        )}
        {viewOptions.length > 1 && (
          <SegmentedControl
            ariaLabel="Calendar view"
            options={viewOptions}
            value={view}
            onChange={onViewChange}
          />
        )}
        {/* The "Calendars x/x" dropdown used to sit here. It now lives in the left sidebar, listing
            every calendar inline instead of behind a button — one control, one home, so it is NOT
            mirrored back into the header. */}
        <Button
          variant="secondary"
          onClick={onRefresh}
          disabled={syncing}
          // The exact moment always lives in the tooltip, even at Minimal where the inline stamp is
          // hidden — so the detail is never unreachable, only quieter.
          title={lastSync ? `Last synced ${formatWhen(lastSync)}` : "Never synced"}
        >
          {syncing ? "Refreshing…" : showMeta && synced ? `Refresh · ${synced}` : "Refresh"}
        </Button>
      </div>
    </header>
  );
}
