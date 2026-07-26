// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Local, per-device UI preferences for the Pinboard — deliberately NOT board state. The board itself
// is user content and lives in the encrypted store (see usePinboard); this is a nag the user has
// waved away, which is a habit of this machine and shouldn't travel with a backup or a vault move.
// localStorage, try/catch on every access, mirroring src/lib/calendarPrefs.ts.
//
// Read at the point of use, never cached: Settings is an overlay ON TOP of a still-mounted
// PinboardView (App's `showSettings` is independent of `view`), so a cached copy would go stale the
// moment the toggle was flipped with the board still behind it.

const CONFIRM_DELETE_KEY = "pm.pinboard.confirmDelete";

/** Ask before deleting a note or timeline? On by default — the first time you delete something must
 *  not be the time you find out there was no confirmation. Only an explicit "don't ask again" turns
 *  it off, and Settings turns it back on. */
export function readConfirmDelete(): boolean {
  try {
    return localStorage.getItem(CONFIRM_DELETE_KEY) !== "false";
  } catch {
    // Unavailable (locked-down webview) — fail SAFE: keep asking.
    return true;
  }
}

export function writeConfirmDelete(on: boolean): void {
  try {
    localStorage.setItem(CONFIRM_DELETE_KEY, on ? "true" : "false");
  } catch {
    // Best-effort; a failed write just means the choice isn't remembered on this device.
  }
}

const SHOW_PAST_KEY = "pm.pinboard.timeline.showPast";

/** Show entries whose date has already gone by, on a timeline card? True by default — same reasoning
 *  as the project panel's "Completed" checkbox: opening a card must not look like it lost data.
 *
 *  Deliberately keyed on the DATE, not on whether a milestone is marked done. The project panel hides
 *  *completed* work because there the question is "what's left to do"; a timeline is a picture of
 *  when things happen, so what crowds it is everything already behind you — including things that
 *  passed without being ticked off. A separate pref from the panel's for the same reason: they answer
 *  different questions, and one switch would make each of them wrong half the time. */
export function readShowPastTimelineItems(): boolean {
  try {
    return localStorage.getItem(SHOW_PAST_KEY) !== "false";
  } catch {
    return true;
  }
}

export function writeShowPastTimelineItems(show: boolean): void {
  try {
    localStorage.setItem(SHOW_PAST_KEY, show ? "true" : "false");
  } catch {
    /* best-effort */
  }
}

/**
 * Whether a timeline entry's date is already behind us. Compared as `YYYY-MM-DD` STRINGS against
 * today's local calendar day rather than by constructing a `Date`: a bare date parsed by `new Date`
 * is read as UTC midnight, which lands on the previous day west of Greenwich (F-14) — so a milestone
 * due today would read as past for a whole timezone's worth of users.
 *
 * Undated entries are never past. They have no position on the timeline to have gone by, and hiding
 * them would silently swallow the ones a user has typed but not dated yet.
 */
export function isPastTimelineDate(date: string | null | undefined, todayIso: string): boolean {
  if (!date) return false;
  const day = date.slice(0, 10);
  return /^\d{4}-\d{2}-\d{2}$/.test(day) && day < todayIso;
}
