// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Every per-device view pref for the Focus tab: the layout, the "Upcoming" section's display mode /
// hour window / day count, and which panels the tab shows. Display-only with no backend consumer, so
// they live in localStorage — never a backend Setting (mirrors mapPrefs).
//
// All of these are SET ON THE FOCUS TAB, beside the thing they change. Settings used to mirror the
// first four; the mirrors are gone (they were a second place to keep in step, and the layout one had
// drifted out of step already). What Settings keeps is `focusViewPrefsAreDefault` /
// `resetFocusViewPrefs`, so "Reset Focus" still reaches everything — the defaults live here once.
//
// Every writer announces on the app-wide `pm:settings-changed` signal. Settings renders as an overlay
// over a still-mounted Focus tab, so both sides have to be able to follow the other's writes rather
// than trusting a read taken at mount.

import { parseRangeBounds, type CalendarRange, type RangeBounds } from "./calendarPrefs";

export type FocusLayout = "split" | "vertical";

export const FOCUS_LAYOUT_KEY = "pm.focus.layout";

/** The app-wide "a setting changed" signal (also used for the panel set below). */
const CHANGED_EVENT = "pm:settings-changed";

function announce(): void {
  try {
    window.dispatchEvent(new Event(CHANGED_EVENT));
  } catch {
    /* non-browser context (tests) */
  }
}

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
  announce();
}

// --- "Upcoming" section: agenda list vs a small few-day calendar grid ----------------------------
// The Focus "Upcoming" card can render either the plain agenda list (the default) or a compact
// day-by-day time grid — the same engine the Calendar tab's Week view uses, capped to a few days so
// it fits the Focus column at the same width. The `range` reuses the calendar's Work/Day vocabulary
// (imported at the top), and so do its editable hour windows — but the STORE is this card's own
// (UPCOMING_BOUNDS_KEY): a ~26rem pane wants a tighter Work window than a full-page week grid, so
// narrowing one here must not narrow the Calendar tab too.

/** How the Upcoming section is drawn. "week" is the few-day grid; "list" (default) is the agenda. */
export type FocusUpcomingMode = "list" | "week";

const UPCOMING_MODE_KEY = "pm.focus.upcoming.mode";
const UPCOMING_RANGE_KEY = "pm.focus.upcoming.range";
const UPCOMING_DAYS_KEY = "pm.focus.upcoming.days";
const UPCOMING_BOUNDS_KEY = "pm.focus.upcoming.bounds";

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
  announce();
}

/** The hour windows the Upcoming grid offers. The calendar's 24h is deliberately absent: this pane is
 *  ~26rem tall, so a whole day's rows can't hold a legible event card. Nothing becomes unreachable —
 *  the grid always spans the full 24h and scrolls to the rest. */
export const FOCUS_UPCOMING_RANGES: readonly CalendarRange[] = ["work", "day"];

/** The hour-window preset for the Upcoming grid (Work / Day). Defaults to the everyday Day — as does
 *  a stored "full" from when this pane still offered 24h, so an existing choice lands somewhere
 *  sensible instead of on a control with nothing selected. */
export function readFocusUpcomingRange(): CalendarRange {
  try {
    const raw = localStorage.getItem(UPCOMING_RANGE_KEY);
    if (FOCUS_UPCOMING_RANGES.some((r) => r === raw)) return raw as CalendarRange;
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
  announce();
}

/** The Upcoming grid's OWN custom Work/Day hour windows — the Calendar tab's editor, its own store
 *  (see UPCOMING_BOUNDS_KEY). An absent key falls back to the computed default (Work 08:30–17:30,
 *  Day = local sunrise/sunset), exactly as the Calendar tab's does. */
export function readFocusUpcomingBounds(): Partial<Record<CalendarRange, RangeBounds>> {
  try {
    return parseRangeBounds(localStorage.getItem(UPCOMING_BOUNDS_KEY));
  } catch {
    return {};
  }
}

export function writeFocusUpcomingBounds(map: Partial<Record<CalendarRange, RangeBounds>>): void {
  try {
    localStorage.setItem(UPCOMING_BOUNDS_KEY, JSON.stringify(map));
  } catch {
    /* best-effort */
  }
  announce();
}

/** Every day-count the Upcoming grid offers, in order — the control and the clamp below read the same
 *  list, so an offered count can never be one the clamp rejects. */
export const FOCUS_UPCOMING_DAY_CHOICES: number[] = Array.from(
  { length: FOCUS_UPCOMING_MAX_DAYS - FOCUS_UPCOMING_MIN_DAYS + 1 },
  (_, i) => FOCUS_UPCOMING_MIN_DAYS + i,
);

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
  announce();
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
  announce();
}

/** True when no panel is hidden. */
export function focusPanelsAreDefault(): boolean {
  return readFocusHiddenPanels().size === 0;
}

export function resetFocusPanels(): void {
  writeFocusHiddenPanels(new Set());
}

// --- the whole Focus-tab view state, for Settings' "Reset Focus" ---------------------------------
// The controls live on the Focus tab, but Settings still owns the reset — so it needs to ask "is any
// of it non-default?" and "put all of it back" without re-listing the defaults. Both live here, with
// the defaults they compare against, so the two can't drift.

/** True when every Focus-tab view pref (layout, Upcoming mode / hour window / day count, panel
 *  visibility) is untouched — drives whether Settings offers "Reset Focus". */
export function focusViewPrefsAreDefault(): boolean {
  return (
    readFocusLayout() === "split" &&
    readFocusUpcomingMode() === "list" &&
    readFocusUpcomingRange() === "day" &&
    readFocusUpcomingDays() === FOCUS_UPCOMING_DEFAULT_DAYS &&
    Object.keys(readFocusUpcomingBounds()).length === 0 &&
    focusPanelsAreDefault()
  );
}

/** Put every Focus-tab view pref back to its default. */
export function resetFocusViewPrefs(): void {
  writeFocusLayout("split");
  writeFocusUpcomingMode("list");
  writeFocusUpcomingRange("day");
  writeFocusUpcomingDays(FOCUS_UPCOMING_DEFAULT_DAYS);
  writeFocusUpcomingBounds({});
  resetFocusPanels();
}
