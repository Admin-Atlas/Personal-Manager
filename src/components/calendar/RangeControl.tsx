// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Work / Day / 24h time-grid range control. Replaces the plain SegmentedControl so Work and Day
// can each carry a ▾ that opens a small popover to edit their visible-hour window (24h is fixed, so
// it has no chevron). Editing a range also selects it, so the grid reflects the change live. Values
// are decimal hours, displayed as 24h HH:MM so the vocabulary stays locale-independent.
//
// The bounds are picked from half-hour <Select> slots, NOT typed into <input type="time">. Blink
// implements the native time widget and only ever emits a complete "HH:MM", but WebKitGTK (Linux)
// has no such widget and degrades it to a plain text box — so every intermediate keystroke failed to
// parse, the controlled value snapped back, and a two-digit hour could never be entered. A select
// can only ever hold a whole valid value, so it behaves identically on both engines. The option
// lists are also cross-filtered to keep at least an hour between the bounds, which is the one window
// sanitizeBounds refuses: every offered slot is one it will accept, so a pick can never silently
// no-op. See ui/Select for the matching WebKitGTK sizing note.
//
// The Focus tab's "Upcoming" grid renders this too, narrowed to Work/Day (`ranges`) and pointed at
// its own bounds store — same editor, same vocabulary, independent windows.

import { sanitizeBounds, type CalendarRange, type RangeBounds } from "../../lib/calendarPrefs";
import { resolveRangeBounds } from "../../lib/calendarGeom";
import type { Coords } from "../../theme";
import { cn } from "../ui";
import { Popover, Select } from "../ui";

const ITEMS: ReadonlyArray<{ value: CalendarRange; label: string; editable: boolean }> = [
  { value: "work", label: "Work", editable: true },
  { value: "day", label: "Day", editable: true },
  { value: "full", label: "24h", editable: false },
];

/** Decimal hour → "HH:MM". 24 renders as "24:00" — end-of-day, which a time input can't express. */
function hoursToHM(h: number): string {
  const hh = Math.floor(h);
  const mm = Math.round((h - hh) * 60);
  return `${String(hh).padStart(2, "0")}:${String(mm).padStart(2, "0")}`;
}

/** Half-hour slots over [lo, hi] inclusive — the granularity sanitizeBounds' round05 already pins. */
function slots(lo: number, hi: number): number[] {
  const out: number[] = [];
  for (let h = lo; h <= hi + 1e-9; h += 0.5) out.push(Math.round(h * 2) / 2);
  return out;
}

/** The narrowest window the geometry accepts, mirroring sanitizeBounds' `endHour - startHour < 1`. */
const MIN_WINDOW_H = 1;

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
  // Cross-filtered so the pair can never form a window sanitizeBounds would reject.
  const startSlots = slots(0, 23.5).filter((h) => h <= effective.endHour - MIN_WINDOW_H);
  const endSlots = slots(0.5, 24).filter((h) => h >= effective.startHour + MIN_WINDOW_H);
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
          <Select
            compact
            value={String(effective.startHour)}
            onChange={(e) => {
              const b = sanitizeBounds({
                startHour: Number(e.target.value),
                endHour: effective.endHour,
              });
              if (b) onApply(b);
            }}
            className="font-mono"
          >
            {startSlots.map((h) => (
              <option key={h} value={h}>
                {hoursToHM(h)}
              </option>
            ))}
          </Select>
        </label>
        <label className="flex items-center justify-between gap-3 text-xs text-ink2">
          <span>End</span>
          <Select
            compact
            value={String(effective.endHour)}
            onChange={(e) => {
              const b = sanitizeBounds({
                startHour: effective.startHour,
                endHour: Number(e.target.value),
              });
              if (b) onApply(b);
            }}
            className="font-mono"
          >
            {endSlots.map((h) => (
              <option key={h} value={h}>
                {hoursToHM(h)}
              </option>
            ))}
          </Select>
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
