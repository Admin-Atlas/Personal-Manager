// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Work / Day / 24h time-grid range control. Replaces the plain SegmentedControl so Work and Day
// can each carry a ▾ that opens a small popover to edit their visible-hour window (24h is fixed, so
// it has no chevron). Editing a range also selects it, so the grid reflects the change live. Values
// are decimal hours; the <input type="time"> exchange is locale-independent (its value is always
// 24h HH:MM). Token-driven; no colours of its own.
//
// The Focus tab's "Upcoming" grid renders this too, narrowed to Work/Day (`ranges`) and pointed at
// its own bounds store — same editor, same vocabulary, independent windows.

import { sanitizeBounds, type CalendarRange, type RangeBounds } from "../../lib/calendarPrefs";
import { resolveRangeBounds } from "../../lib/calendarGeom";
import type { Coords } from "../../theme";
import { cn } from "../ui";
import { Popover } from "../ui";

const ITEMS: ReadonlyArray<{ value: CalendarRange; label: string; editable: boolean }> = [
  { value: "work", label: "Work", editable: true },
  { value: "day", label: "Day", editable: true },
  { value: "full", label: "24h", editable: false },
];

/** Decimal hour → "HH:MM" for the time input, capped at 23:30 (the input can't express 24:00). */
function hoursToHM(h: number): string {
  const clamped = Math.max(0, Math.min(23.5, h));
  const hh = Math.floor(clamped);
  const mm = Math.round((clamped - hh) * 60);
  return `${String(hh).padStart(2, "0")}:${String(mm).padStart(2, "0")}`;
}

/** "HH:MM" → decimal hour, or null if unparseable. */
function hmToHours(v: string): number | null {
  const m = /^(\d{1,2}):(\d{2})$/.exec(v);
  if (!m) return null;
  const h = Number(m[1]) + Number(m[2]) / 60;
  return Number.isFinite(h) ? h : null;
}

interface Props {
  range: CalendarRange;
  onRangeChange: (r: CalendarRange) => void;
  customBounds: Partial<Record<CalendarRange, RangeBounds>>;
  /** Persist a custom window for a range, or `null` to clear it back to the computed default. */
  onBoundsChange: (range: CalendarRange, bounds: RangeBounds | null) => void;
  coords: Coords | null;
  /** The anchor day, so the Day default reflects sunrise/sunset for the shown date. */
  cursor: Date;
  /** Which ranges to offer, in order. Defaults to all three; the Focus tab's Upcoming pane passes
   *  `["work", "day"]` because at ~26rem tall a 24h grid can't hold a legible event card. */
  ranges?: readonly CalendarRange[];
}

export function RangeControl({
  range,
  onRangeChange,
  customBounds,
  onBoundsChange,
  coords,
  cursor,
  ranges,
}: Props) {
  const items = ranges ? ITEMS.filter((it) => ranges.includes(it.value)) : ITEMS;
  return (
    <div
      className="inline-flex items-center gap-0.5 rounded-[var(--radius-sm)] border border-border2 p-0.5"
      role="group"
      aria-label="Time-grid hours"
    >
      {items.map((it) => {
        const active = it.value === range;
        return (
          <div
            key={it.value}
            className={cn("flex items-center rounded-[var(--radius-sm)]", active && "bg-accent")}
          >
            <button
              type="button"
              aria-pressed={active}
              onClick={() => onRangeChange(it.value)}
              className={cn(
                "px-2.5 py-1 text-xs transition",
                active ? "font-medium text-accent-ink" : "text-ink3 hover:text-ink",
                it.editable ? "rounded-l-[var(--radius-sm)]" : "rounded-[var(--radius-sm)]",
              )}
            >
              {it.label}
            </button>
            {it.editable && (
              <RangeEditor
                rangeKey={it.value}
                active={active}
                effective={resolveRangeBounds(it.value, customBounds, coords, cursor)}
                isCustom={!!customBounds[it.value]}
                onApply={(b) => {
                  onBoundsChange(it.value, b);
                  onRangeChange(it.value);
                }}
                onReset={() => {
                  onBoundsChange(it.value, null);
                  onRangeChange(it.value);
                }}
              />
            )}
          </div>
        );
      })}
    </div>
  );
}

function RangeEditor({
  rangeKey,
  active,
  effective,
  isCustom,
  onApply,
  onReset,
}: {
  rangeKey: CalendarRange;
  active: boolean;
  effective: RangeBounds;
  isCustom: boolean;
  onApply: (b: RangeBounds) => void;
  onReset: () => void;
}) {
  const inputCls =
    "rounded-[var(--radius-sm)] border border-border2 bg-surface px-1.5 py-0.5 font-mono text-xs text-ink2 focus:border-accent focus:outline-none";
  return (
    <Popover
      align="right"
      ariaLabel={`${rangeKey} hours`}
      panelClassName="min-w-[13rem] p-3"
      trigger={({ toggle }) => (
        <button
          type="button"
          onClick={toggle}
          title="Set hours"
          aria-label={`Set ${rangeKey} hours`}
          className={cn(
            "inline-flex items-center justify-center min-h-[var(--tap-min,24px)] min-w-[var(--tap-min,24px)] rounded-r-[var(--radius-sm)] text-[0.625rem] transition",
            active ? "text-accent-ink" : "text-ink4 hover:text-ink",
          )}
        >
          ▾
        </button>
      )}
    >
      <div className="space-y-2">
        <label className="flex items-center justify-between gap-3 text-xs text-ink2">
          <span>Start</span>
          <input
            type="time"
            step={1800}
            value={hoursToHM(effective.startHour)}
            onChange={(e) => {
              const s = hmToHours(e.target.value);
              if (s == null) return;
              const b = sanitizeBounds({ startHour: s, endHour: effective.endHour });
              if (b) onApply(b);
            }}
            className={inputCls}
          />
        </label>
        <label className="flex items-center justify-between gap-3 text-xs text-ink2">
          <span>End</span>
          <input
            type="time"
            step={1800}
            value={hoursToHM(effective.endHour)}
            onChange={(e) => {
              const en = hmToHours(e.target.value);
              if (en == null) return;
              const b = sanitizeBounds({ startHour: effective.startHour, endHour: en });
              if (b) onApply(b);
            }}
            className={inputCls}
          />
        </label>
        {isCustom && (
          <button
            type="button"
            onClick={onReset}
            className="text-[0.6875rem] text-accent-text hover:underline"
          >
            Reset to default
          </button>
        )}
      </div>
    </Popover>
  );
}
