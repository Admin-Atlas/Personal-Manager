// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { isLetterKey } from "./letterKey";

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

/** The parts of a keyboard event the Undo decision reads, on top of {@link NavKeyEvent}. */
export interface UndoKeyEvent extends NavKeyEvent {
  shiftKey: boolean;
  /** The physical key ("KeyZ"), for a layout that types no Latin letter ({@link isLetterKey}). */
  code: string;
  repeat: boolean;
}

/** Whether a key went to something being typed in, whose own keys (and own Undo) come first. */
export function isTypingTarget(target: EventTarget | null): boolean {
  const t = target as HTMLElement | null;
  return !!t && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName ?? ""));
}

/**
 * The Calendar tab's keyboard shortcuts (← / → / `t`), as a pure decision. Nothing when a modifier is
 * held (app shortcuts), when typing in a field, or when a dialog is open: the listener sits on
 * `window`, so without the dialog check a key pressed on a dialog's button paged the grid behind it.
 */
export function calendarNavKey(e: NavKeyEvent, dialogOpen: boolean): CalendarNav | null {
  if (dialogOpen) return null;
  if (e.metaKey || e.ctrlKey || e.altKey) return null;
  if (isTypingTarget(e.target)) return null;
  if (e.key === "ArrowLeft") return "prev";
  if (e.key === "ArrowRight") return "next";
  if (e.key === "t" || e.key === "T") return "today";
  return null;
}

/**
 * Whether a key press is the Calendar tab's Undo: Ctrl+Z, or Cmd+Z on a Mac (either works on any
 * computer, as on the Pinboard), which presses the newest Undo on screen (#884, F5; Bobby: "can we
 * make control z also undo it within 8 seconds"). It stands down where the arrow keys do: in a field,
 * which has its own Undo for its text, and behind a dialog. Not with Shift (redo has nothing to redo:
 * deleting again goes through its confirmation), with Alt, or for a held-down key, which would undo
 * one delete after another.
 */
export function calendarUndoKey(e: UndoKeyEvent, dialogOpen: boolean): boolean {
  if (dialogOpen || e.repeat) return false;
  if (!(e.ctrlKey || e.metaKey) || e.altKey || e.shiftKey) return false;
  if (isTypingTarget(e.target)) return false;
  return isLetterKey(e, "z");
}
