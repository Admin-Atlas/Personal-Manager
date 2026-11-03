// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The calendar's shared chrome — one header, no second sidebar. ‹ Today › nav first, at the far left
// where the eye starts, then the date-range label, the view switcher (active = accent), and
// Refresh-now. In Agenda, a Runway | List switch sits beside the view switcher; the Runway always
// runs from today, so with it on, the nav and the date label give way to "From today".
//
// "+ New event" (#884, C10) is here only in the views without a grid corner to hold it (Month, Year,
// Agenda, and Terminal's), at the right end beside Refresh; in Week and Day it sits in the grid's
// top-left corner instead (see `NewEventButton`). It shows once a Google account is connected.
// Everything else here only changes what you see.
//
// The last-synced time rides ON the Refresh button rather than beside it: it is the one fact that
// tells you whether pressing Refresh is worth it.

import { type Ref } from "react";
import {
  DAY_COUNT_MAX,
  DAY_COUNT_MIN,
  type AgendaMode,
  type CalendarRange,
  type CalendarViewMode,
  type RangeBounds,
} from "../../lib/calendarPrefs";
import { formatSyncedShort, formatWhen } from "../../lib/format";
import { useDepth, useTheme, type Coords } from "../../theme";
import { Button, SegmentedControl, Select, type SegOption } from "../ui";
import { MiniCalendarPopover } from "./MiniCalendarPopover";
import { NewEventButton } from "./NewEventButton";
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
  /** Start a new event. Absent: no Google account is connected, or the button is in the grid's
   *  corner (Week and Day), and there's none here. */
  onCreate?: () => void;
  /** Why no new event can be made now, or null when one can. */
  createReason?: string | null;
  /** The "+ New event" button: quick create opens under it, and gives focus back to it. */
  createRef?: Ref<HTMLButtonElement>;
  /** The Agenda's layout: the Runway board or the dated list. Its switch shows in Agenda only. */
  agendaMode?: AgendaMode;
  onAgendaModeChange?: (mode: AgendaMode) => void;
}

const AGENDA_OPTIONS: SegOption<AgendaMode>[] = [
  { value: "runway", label: "Runway", title: "Your tasks in columns, from today on" },
  { value: "list", label: "List", title: "Your events, day by day" },
];

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
  onCreate,
  createReason = null,
  createRef,
  agendaMode = "list",
  onAgendaModeChange,
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
  // The Runway always runs from today, so paging and the date picker have nothing to move there.
  const runway = view === "agenda" && agendaMode === "runway" && !!onAgendaModeChange;

  return (
    <header
      className="flex flex-wrap items-center gap-x-4 gap-y-2 border-b border-rule px-4 py-2"
      data-help="calendar-header"
    >
      {runway ? (
        <span className="px-1 font-head text-sm text-ink">From today</span>
      ) : (
        <>
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
        </>
      )}

      <div className="ml-auto flex flex-wrap items-center gap-3">
        {view === "agenda" && onAgendaModeChange && (
          // Agenda only, beside the view switcher: it changes how this one view lays out, so it
          // sits by what it changes and nowhere else (one control, one home).
          <SegmentedControl
            ariaLabel="Agenda layout"
            options={AGENDA_OPTIONS}
            value={agendaMode}
            onChange={onAgendaModeChange}
          />
        )}
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
        {onCreate && (
          <NewEventButton onCreate={onCreate} createReason={createReason} createRef={createRef} />
        )}
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
