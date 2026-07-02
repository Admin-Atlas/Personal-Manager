// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The calendar's shared chrome — one header, no second sidebar. View switcher (active = accent),
// ‹ Today › nav, the date-range label, the Calendars dropdown, Refresh-now, and a read-only
// indicator. Meta (sync time) is gated by depth; the layout never forks.

import type { Calendar, CalendarAccount } from "../../lib/types";
import type { CalendarRange, CalendarViewMode } from "../../lib/calendarPrefs";
import { useDepth } from "../../theme";
import { Button, SegmentedControl, type SegOption } from "../ui";
import { CalendarsDropdown } from "./CalendarsDropdown";
import { MiniCalendarPopover } from "./MiniCalendarPopover";

interface Props {
  view: CalendarViewMode;
  availableViews: readonly CalendarViewMode[];
  onViewChange: (v: CalendarViewMode) => void;
  /** Time-grid vertical scale (Week/Day only). */
  range: CalendarRange;
  onRangeChange: (r: CalendarRange) => void;
  /** The current period label (e.g. "July 2026") shown between the nav arrows. */
  label: string;
  /** The anchor day, for the mini-calendar picker. */
  cursor: Date;
  onPickDate: (d: Date) => void;
  onPrev: () => void;
  onNext: () => void;
  onToday: () => void;
  accounts: CalendarAccount[];
  calendars: Calendar[];
  hidden: Set<string>;
  onToggleCalendar: (calendarId: string) => void;
  colorOf: (calendarId: string) => string;
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

const RANGE_OPTIONS: SegOption<CalendarRange>[] = [
  { value: "work", label: "Work" },
  { value: "day", label: "Day" },
  { value: "full", label: "24h" },
];

/** Local clock time of the last successful sync, or null. */
function syncLabel(iso: string | null): string | null {
  if (!iso) return null;
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return null;
  return d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
}

export function CalendarHeader({
  view,
  availableViews,
  onViewChange,
  range,
  onRangeChange,
  label,
  cursor,
  onPickDate,
  onPrev,
  onNext,
  onToday,
  accounts,
  calendars,
  hidden,
  onToggleCalendar,
  colorOf,
  onRefresh,
  syncing,
  lastSync,
}: Props) {
  const { showMeta } = useDepth();
  const synced = syncLabel(lastSync);
  const showRange = view === "week" || view === "day";

  const viewOptions: SegOption<CalendarViewMode>[] = availableViews.map((v) => ({
    value: v,
    label: VIEW_LABEL[v],
  }));

  return (
    <header className="flex flex-wrap items-center gap-x-4 gap-y-2 border-b border-rule px-4 py-2">
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
        {showRange && (
          <SegmentedControl options={RANGE_OPTIONS} value={range} onChange={onRangeChange} />
        )}
        {viewOptions.length > 1 && (
          <SegmentedControl options={viewOptions} value={view} onChange={onViewChange} />
        )}
        <CalendarsDropdown
          accounts={accounts}
          calendars={calendars}
          hidden={hidden}
          onToggle={onToggleCalendar}
          colorOf={colorOf}
        />
        <Button variant="secondary" onClick={onRefresh} disabled={syncing} title="Refresh now">
          {syncing ? "Refreshing…" : "Refresh"}
        </Button>
        <div className="flex items-center gap-2">
          <span className="rounded-[var(--radius-sm)] border border-border px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-wide text-ink4">
            Read-only
          </span>
          {showMeta && synced && (
            <span className="font-mono text-xs text-ink4">synced {synced}</span>
          )}
        </div>
      </div>
    </header>
  );
}
