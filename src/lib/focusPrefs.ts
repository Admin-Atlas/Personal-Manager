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

// --- which panels the Focus tab shows -----------------------------------------------------------
// The Focus tab is a stack of self-contained panels, and not everyone wants all of them. This is the
// user's choice of which to show, in the same "widget" spirit as the calendar's per-calendar
// visibility — and it borrows that module's shape deliberately (calendarPrefs readHidden/writeHidden).
//
// We store what is HIDDEN, not what is visible. That way the default is "everything shows" with no
// stored state to migrate, and any panel added later ships visible rather than silently absent for
// everyone who already has a stored preference.

/** The Focus panels a user can switch off. The header is deliberately absent: it carries the control
 *  that brings a panel back, so hiding it would strand the user with no way to undo. */
export type FocusPanel = "briefing" | "actions" | "upcoming" | "projects";

/** Display order + labels, shared by the Focus header control and any other lister. */
export const FOCUS_PANELS: { id: FocusPanel; label: string }[] = [
  { id: "briefing", label: "Today's briefing" },
  { id: "actions", label: "Focus box" },
  { id: "upcoming", label: "Upcoming" },
  { id: "projects", label: "Projects" },
];

const FOCUS_HIDDEN_KEY = "pm.focus.hidden";
const PANEL_IDS = new Set<string>(FOCUS_PANELS.map((p) => p.id));

/** The app-wide "a setting changed" signal, so a mounted Focus view follows a Settings-side reset. */
const CHANGED_EVENT = "pm:settings-changed";

/** The set of panels the user has switched off. Unreadable or absent ⇒ nothing hidden. */
export function readFocusHiddenPanels(): Set<FocusPanel> {
  try {
    const raw = localStorage.getItem(FOCUS_HIDDEN_KEY);
    if (raw) {
      const arr: unknown = JSON.parse(raw);
      if (Array.isArray(arr)) {
        // Filtered against the known ids, so a stale id from an older build can't hide a panel that
        // no longer answers to that name.
        return new Set(
          arr.filter((x): x is FocusPanel => typeof x === "string" && PANEL_IDS.has(x)),
        );
      }
    }
  } catch {
    /* nothing hidden */
  }
  return new Set();
}

export function writeFocusHiddenPanels(hidden: Set<FocusPanel>): void {
  try {
    localStorage.setItem(FOCUS_HIDDEN_KEY, JSON.stringify([...hidden]));
  } catch {
    /* best-effort */
  }
  try {
    window.dispatchEvent(new Event(CHANGED_EVENT));
  } catch {
    /* non-browser context (tests) */
  }
}

/** True when no panel is hidden — drives Settings' "Reset Focus" affordance. */
export function focusPanelsAreDefault(): boolean {
  return readFocusHiddenPanels().size === 0;
}

export function resetFocusPanels(): void {
  writeFocusHiddenPanels(new Set());
}
