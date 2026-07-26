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
  // Two surfaces render this set at once — the sidebar block and the calendar grid — and the sidebar
  // never unmounts, so a tick in one has to reach the other. The repo's established cross-surface
  // signal rather than a second bespoke one (mirrors focusPrefs / briefingPrefs).
  try {
    window.dispatchEvent(new Event("pm:settings-changed"));
  } catch {
    /* non-browser context (tests) */
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

/** Validate a stored bounds blob into a per-range map. Only `work`/`day` are honoured (24h is fixed);
 *  anything unparseable, missing or nonsensical is simply absent, and an absent key falls back to the
 *  computed default (see calendarGeom.resolveRangeBounds).
 *
 *  Key-free so the Focus tab's Upcoming grid can reuse the validator while keeping its OWN windows
 *  under its own key — one definition of "a valid hour window", two independent stores. */
export function parseRangeBounds(raw: string | null): Partial<Record<CalendarRange, RangeBounds>> {
  try {
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

/** The Calendar tab's custom bounds per range. */
export function readRangeBounds(): Partial<Record<CalendarRange, RangeBounds>> {
  try {
    return parseRangeBounds(localStorage.getItem(RANGE_BOUNDS_KEY));
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

// --- the Day view's width, and where the calendar opens ------------------------------------------

const DAY_COUNT_KEY = "pm.calendar.dayCount";
const OPEN_ON_KEY = "pm.calendar.openOn";
const CURSOR_KEY = "pm.calendar.cursor";

/** How many days the Day view may show. Capped at 6 on purpose: 7 IS the Week view, and letting Day
 *  reach it would give two controls that produce the same picture and disagree about what "Today"
 *  means. */
export const DAY_COUNT_MIN = 1;
export const DAY_COUNT_MAX = 6;

export function clampDayCount(n: number): number {
  if (!Number.isFinite(n)) return DAY_COUNT_MIN;
  return Math.max(DAY_COUNT_MIN, Math.min(DAY_COUNT_MAX, Math.trunc(n)));
}

/** How many days the Day view shows. Defaults to 1 — the view's historical behaviour, so nobody's
 *  calendar changes shape on upgrade. */
export function readDayCount(): number {
  try {
    const raw = localStorage.getItem(DAY_COUNT_KEY);
    return raw ? clampDayCount(Number(raw)) : DAY_COUNT_MIN;
  } catch {
    return DAY_COUNT_MIN;
  }
}

export function writeDayCount(n: number): void {
  try {
    localStorage.setItem(DAY_COUNT_KEY, String(clampDayCount(n)));
  } catch {
    // Best-effort.
  }
}

/** Whether the calendar opens on today or wherever it was left. */
export type CalendarOpenOn = "today" | "last";

/** Defaults to `today`: opening on a date you last looked at weeks ago, with no memory of having
 *  left it there, reads as the calendar being broken. Opt in from Settings. */
export function readOpenOn(): CalendarOpenOn {
  try {
    return localStorage.getItem(OPEN_ON_KEY) === "last" ? "last" : "today";
  } catch {
    return "today";
  }
}

export function writeOpenOn(mode: CalendarOpenOn): void {
  try {
    localStorage.setItem(OPEN_ON_KEY, mode);
  } catch {
    // Best-effort.
  }
}

/** The cursor day, stored as `YYYY-MM-DD`. Written on every move regardless of the `openOn` setting
 *  — so turning "where I left off" on starts working immediately rather than after the next move. */
export function readCursorDay(): Date | null {
  try {
    const raw = localStorage.getItem(CURSOR_KEY);
    const m = raw && /^(\d{4})-(\d{2})-(\d{2})$/.exec(raw);
    if (!m) return null;
    // Built from components, not `new Date(raw)`, which reads a bare date as UTC midnight and lands
    // on the previous day west of Greenwich (F-14).
    const d = new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3]));
    return Number.isNaN(d.getTime()) ? null : d;
  } catch {
    return null;
  }
}

export function writeCursorDay(d: Date): void {
  try {
    const p2 = (n: number) => String(n).padStart(2, "0");
    localStorage.setItem(
      CURSOR_KEY,
      `${d.getFullYear()}-${p2(d.getMonth() + 1)}-${p2(d.getDate())}`,
    );
  } catch {
    // Best-effort.
  }
}
