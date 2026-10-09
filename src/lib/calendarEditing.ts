// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { EditingStatus } from "./types";

/**
 * The accounts (by id) whose editing a connect just turned off. Connecting an account again asks
 * Google for reading only, so it clears editing even on an account that had it; this finds them so
 * the note after the connect can say so instead of editing quietly disappearing. An account missing
 * from `after` was removed, not turned off, so it isn't listed.
 */
export function editingTurnedOff(
  before: Record<string, EditingStatus>,
  after: Record<string, EditingStatus>,
): string[] {
  return Object.entries(before)
    .filter(([id, was]) => was !== "off" && after[id] === "off")
    .map(([id]) => id);
}
