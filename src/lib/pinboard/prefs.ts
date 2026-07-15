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
