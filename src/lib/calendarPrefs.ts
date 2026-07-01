// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Local, per-device VIEW state for the unified calendar (card 8) — deliberately NOT feature state.
// Which calendars *sync* into the mirror is `calendars.selected` (owned by Connectors settings);
// this only remembers the view the user last had open and which of those synced calendars they've
// hidden from the aggregator. Hiding is instant and never re-syncs or purges. Mirrors the
// `pm.focus.sort` localStorage pattern in FocusView.

/** The calendar's view modes. Day = the N-day time grid with N=1 (see PR2). */
export type CalendarViewMode = "month" | "week" | "day" | "year" | "agenda";

export const CALENDAR_VIEW_MODES: readonly CalendarViewMode[] = [
  "month",
  "week",
  "day",
  "year",
  "agenda",
];

const VIEW_KEY = "pm.calendar.view";
const HIDDEN_KEY = "pm.calendar.hidden";

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
