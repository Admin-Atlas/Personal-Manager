// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Local, per-device VIEW state for the unified calendar (card 8) — deliberately NOT feature state.
// Which calendars *sync* into the mirror is `calendars.selected` (owned by Connectors settings);
// this only remembers the view the user last had open and which of those synced calendars they've
// hidden from the aggregator. Hiding is instant and never re-syncs or purges. Mirrors the
// `pm.focus.sort` localStorage pattern in FocusView.

import { isValidTimeZone } from "../theme/timezones";

/** The calendar's view modes. Day = the N-day time grid with N=1 (see PR2). */
export type CalendarViewMode = "month" | "week" | "day" | "year" | "agenda";

/** The time-grid vertical scale (Week/Day only): the visible hour band + row height. The grid always
 *  spans a scrollable full 24h — `work` opens tall and scrolled to 08:00 (business hours), `day`
 *  opens framed on 08:00–20:00 with rows stretched to fill the body exactly (a daytime-only *default
 *  view*, not a hard clip — scroll reaches the rest), `full` stretches all 24h to fill the body with
 *  no scroll needed. Maps to a px-per-hour + a fill window in TimeGridView. */
export type CalendarRange = "work" | "day" | "full";

export const CALENDAR_RANGES: readonly CalendarRange[] = ["work", "day", "full"];

const VIEW_KEY = "pm.calendar.view";
const HIDDEN_KEY = "pm.calendar.hidden";
const RANGE_KEY = "pm.calendar.range";

/** The last view the user had open, clamped to `allowed` (so a value from a newer build that isn't
 *  available yet falls back to the first allowed mode). */
export function readView(
  allowed: readonly CalendarViewMode[],
  fallback: CalendarViewMode,
): CalendarViewMode {
  try {
    const raw = localStorage.getItem(VIEW_KEY);
    if (raw && (allowed as readonly string[]).includes(raw)) {
      return raw as CalendarViewMode;
    }
  } catch {
    // localStorage can be unavailable (private mode / disabled) — fall through to the default.
  }
  return fallback;
}

export function writeView(view: CalendarViewMode): void {
  try {
    localStorage.setItem(VIEW_KEY, view);
  } catch {
    // Best-effort; a failed write just means the preference isn't remembered.
  }
}

/** The set of calendar ids the user has hidden from the aggregator (visibility, not sync). */
export function readHidden(): Set<string> {
  try {
    const raw = localStorage.getItem(HIDDEN_KEY);
    if (raw) {
      const arr: unknown = JSON.parse(raw);
      if (Array.isArray(arr)) {
        return new Set(arr.filter((x): x is string => typeof x === "string"));
      }
    }
  } catch {
    // Unreadable/absent — nothing hidden.
  }
  return new Set();
}

export function writeHidden(hidden: Set<string>): void {
  try {
    localStorage.setItem(HIDDEN_KEY, JSON.stringify([...hidden]));
  } catch {
    // Best-effort.
  }
}

/** The last time-grid range the user chose (Week/Day). Defaults to the everyday whole-day grid. */
export function readRange(): CalendarRange {
  try {
    const raw = localStorage.getItem(RANGE_KEY);
    if (raw && (CALENDAR_RANGES as readonly string[]).includes(raw)) {
      return raw as CalendarRange;
    }
  } catch {
    // Unavailable — fall through to the default.
  }
  return "day";
}

export function writeRange(range: CalendarRange): void {
  try {
    localStorage.setItem(RANGE_KEY, range);
  } catch {
    // Best-effort.
  }
}

// --- extra timezones (Week/Day gutter) -----------------------------------------------------------

const ZONES_KEY = "pm.calendar.zones";
/** Up to this many EXTRA timezone columns beside the local one. */
export const MAX_EXTRA_ZONES = 2;

/** The user's chosen extra IANA zones for the gutter (validated, deduped, capped). */
export function readZones(): string[] {
  try {
    const raw = localStorage.getItem(ZONES_KEY);
    if (!raw) return [];
    const arr: unknown = JSON.parse(raw);
    if (!Array.isArray(arr)) return [];
    const seen = new Set<string>();
    const out: string[] = [];
    for (const z of arr) {
      if (typeof z === "string" && isValidTimeZone(z) && !seen.has(z)) {
        seen.add(z);
        out.push(z);
      }
    }
    return out.slice(0, MAX_EXTRA_ZONES);
  } catch {
    return [];
  }
}

export function writeZones(zones: string[]): void {
  try {
    localStorage.setItem(ZONES_KEY, JSON.stringify(zones.slice(0, MAX_EXTRA_ZONES)));
  } catch {
    // Best-effort.
  }
}

// --- custom Work/Day hour bounds -----------------------------------------------------------------

/** A visible hour window as decimal hours (half-hours allowed), e.g. { startHour: 8.5, endHour: 17.5 }. */
export interface RangeBounds {
  startHour: number;
  endHour: number;
}

const RANGE_BOUNDS_KEY = "pm.calendar.rangeBounds";

/** Work defaults to a *display* of 08:30–17:30 — business 9–5 padded so events at 9/5 read clearly. */
export const WORK_DEFAULT: RangeBounds = { startHour: 8.5, endHour: 17.5 };
/** "24h" is fixed to the whole day. */
export const FULL_BOUNDS: RangeBounds = { startHour: 0, endHour: 24 };
/** Day's fallback when sunrise/sunset can't be computed (unknown location / polar day-night). */
export const DAY_FALLBACK: RangeBounds = { startHour: 7, endHour: 19 };

function round05(n: number): number {
  return Math.round(n * 2) / 2;
}
function clamp(n: number, lo: number, hi: number): number {
  return Math.max(lo, Math.min(hi, n));
}

/** Coerce/round/clamp a candidate bound, or null if unusable (non-finite, or a window under 1h). */
export function sanitizeBounds(b: { startHour: unknown; endHour: unknown }): RangeBounds | null {
  const s = Number(b.startHour);
  const e = Number(b.endHour);
  if (!Number.isFinite(s) || !Number.isFinite(e)) return null;
  const startHour = clamp(round05(s), 0, 23.5);
  const endHour = clamp(round05(e), 0.5, 24);
  if (endHour - startHour < 1) return null; // too small / inverted — the geometry needs a real window
  return { startHour, endHour };
}

/** The user's custom bounds per range. Only `work`/`day` are honoured (24h is fixed); absent keys
 *  fall back to the computed defaults (see calendarGeom.resolveRangeBounds). */
export function readRangeBounds(): Partial<Record<CalendarRange, RangeBounds>> {
  try {
    const raw = localStorage.getItem(RANGE_BOUNDS_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as Record<string, unknown>;
    const out: Partial<Record<CalendarRange, RangeBounds>> = {};
    for (const key of ["work", "day"] as const) {
      const v = parsed[key];
      if (v && typeof v === "object") {
        const b = sanitizeBounds(v as { startHour: unknown; endHour: unknown });
        if (b) out[key] = b;
      }
    }
    return out;
  } catch {
    return {};
  }
}

export function writeRangeBounds(map: Partial<Record<CalendarRange, RangeBounds>>): void {
  try {
    localStorage.setItem(RANGE_BOUNDS_KEY, JSON.stringify(map));
  } catch {
    // Best-effort.
  }
}
