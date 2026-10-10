// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// How often PM checks Google for calendar changes made elsewhere (#884, F3). Bobby: "google change,
// pm needs me to press refresh … it should be maybe 30 seconds or at most a minute."
//
// The full calendar sync (every selected calendar's whole band, every iCal feed, every account's list)
// stays at 15 minutes. On top of it, while PM's window is on screen, a cheap check runs about every
// 30 seconds: one small request per Google calendar asking what changed, and a fetch only for the
// calendars that did (`check_google_calendars`). While the window is hidden or in the tray it pauses,
// and the 15-minute sync keeps the briefing and chat fed; bringing PM back checks at once.
//
// The 30 s wobbles by a quarter either way, as Google asks of clients that poll, so checks don't fall
// into step with anything else on the same schedule.

/** The interval the check aims for while PM is on screen. */
export const CHECK_EVERY_MS = 30_000;

/** The wait before the next check: {@link CHECK_EVERY_MS}, give or take a quarter. `random` is in
 *  [0, 1); passed in so the spread can be tested. */
export function nextCheckDelay(random: number = Math.random()): number {
  return Math.round(CHECK_EVERY_MS * (0.75 + 0.5 * random));
}

/** Whether a check should run now: only while the window is on screen. */
export function shouldCheck(visibility: DocumentVisibilityState): boolean {
  return visibility === "visible";
}
