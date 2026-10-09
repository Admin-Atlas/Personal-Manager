// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

/** What a key press asks the Calendar tab to do: step back or forward a period, or jump to today. */
export type CalendarNav = "prev" | "next" | "today";

/** The parts of a keyboard event the decision reads. */
export interface NavKeyEvent {
  key: string;
  metaKey: boolean;
  ctrlKey: boolean;
  altKey: boolean;
  target: EventTarget | null;
}

/**
 * The Calendar tab's keyboard shortcuts (← / → / `t`), as a pure decision. Nothing when a modifier is
 * held (app shortcuts), when typing in a field, or when a dialog is open: the listener sits on
 * `window`, so without the dialog check a key pressed on a dialog's button paged the grid behind it.
 */
export function calendarNavKey(e: NavKeyEvent, dialogOpen: boolean): CalendarNav | null {
  if (dialogOpen) return null;
  if (e.metaKey || e.ctrlKey || e.altKey) return null;
  const t = e.target as HTMLElement | null;
  if (t && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName ?? ""))) return null;
  if (e.key === "ArrowLeft") return "prev";
  if (e.key === "ArrowRight") return "next";
  if (e.key === "t" || e.key === "T") return "today";
  return null;
}
