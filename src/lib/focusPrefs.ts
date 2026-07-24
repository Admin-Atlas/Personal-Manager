// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Per-device layout pref for the Focus tab, shared by the Focus header toggle and the Settings →
// General control (both read/write the same key, so the default lives here once). "split" (the
// default) puts the briefing / actions / agenda beside the project list on a wide screen; "vertical"
// keeps the single stacked column. Display-only with no backend consumer, so it lives in
// localStorage — never a backend Setting (mirrors mapPrefs).

import type { CalendarRange } from "./calendarPrefs";

export type FocusLayout = "split" | "vertical";

export const FOCUS_LAYOUT_KEY = "pm.focus.layout";

/** The stored layout; anything but "vertical" (including absent) is the split default. */
export function readFocusLayout(): FocusLayout {
  try {
    return localStorage.getItem(FOCUS_LAYOUT_KEY) === "vertical" ? "vertical" : "split";
  } catch {
    return "split";
  }
}

export function writeFocusLayout(layout: FocusLayout): void {
  try {
    localStorage.setItem(FOCUS_LAYOUT_KEY, layout);
  } catch {
    /* best-effort — a private-mode / quota failure just means it won't persist */
  }
}

// --- "Upcoming" section: agenda list vs a small few-day calendar grid ----------------------------
// The Focus "Upcoming" card can render either the plain agenda list (the default) or a compact
// day-by-day time grid — the same engine the Calendar tab's Week view uses, capped to a few days so
// it fits the Focus column at the same width. Both the Upcoming header controls and the Settings →
// General → Focus controls read/write these keys, so the defaults live here once (mirrors the layout
// pref above). The `range` reuses the calendar's Work/Day/24h vocabulary (imported at the top).

/** How the Upcoming section is drawn. "week" is the few-day grid; "list" (default) is the agenda. */
export type FocusUpcomingMode = "list" | "week";

const UPCOMING_MODE_KEY = "pm.focus.upcoming.mode";
const UPCOMING_RANGE_KEY = "pm.focus.upcoming.range";
const UPCOMING_DAYS_KEY = "pm.focus.upcoming.days";

/** Narrowest / widest day-grid the Upcoming section will draw — kept small so the columns stay legible
 *  at the Focus column width. */
export const FOCUS_UPCOMING_MIN_DAYS = 1;
export const FOCUS_UPCOMING_MAX_DAYS = 4;
const FOCUS_UPCOMING_DEFAULT_DAYS = 3;

/** The stored Upcoming display mode; anything but "week" (including absent) is the list default. */
export function readFocusUpcomingMode(): FocusUpcomingMode {
  try {
    return localStorage.getItem(UPCOMING_MODE_KEY) === "week" ? "week" : "list";
  } catch {
    return "list";
  }
}

export function writeFocusUpcomingMode(mode: FocusUpcomingMode): void {
  try {
    localStorage.setItem(UPCOMING_MODE_KEY, mode);
  } catch {
    /* best-effort */
  }
}

/** The hour-window preset for the Upcoming grid (Work / Day / 24h). Defaults to the everyday Day. */
export function readFocusUpcomingRange(): CalendarRange {
  try {
    const raw = localStorage.getItem(UPCOMING_RANGE_KEY);
    if (raw === "work" || raw === "day" || raw === "full") return raw;
  } catch {
    /* fall through to the default */
  }
  return "day";
}

export function writeFocusUpcomingRange(range: CalendarRange): void {
  try {
    localStorage.setItem(UPCOMING_RANGE_KEY, range);
  } catch {
    /* best-effort */
  }
}

/** Clamp a candidate day-count into the supported 1–4 window. */
export function clampFocusUpcomingDays(days: number): number {
  return Math.min(FOCUS_UPCOMING_MAX_DAYS, Math.max(FOCUS_UPCOMING_MIN_DAYS, Math.round(days)));
}

/** How many days the Upcoming grid shows (1–4). Defaults to 3. */
export function readFocusUpcomingDays(): number {
  try {
    const n = Number(localStorage.getItem(UPCOMING_DAYS_KEY));
    if (Number.isFinite(n) && n >= FOCUS_UPCOMING_MIN_DAYS && n <= FOCUS_UPCOMING_MAX_DAYS) {
      return Math.round(n);
    }
  } catch {
    /* fall through to the default */
  }
  return FOCUS_UPCOMING_DEFAULT_DAYS;
}

export function writeFocusUpcomingDays(days: number): void {
  try {
    localStorage.setItem(UPCOMING_DAYS_KEY, String(clampFocusUpcomingDays(days)));
  } catch {
    /* best-effort */
  }
}
