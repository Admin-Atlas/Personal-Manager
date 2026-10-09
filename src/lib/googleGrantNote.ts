// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { GoogleUse } from "./types";

/** Google's own page for removing an app's access to an account (named by its native-app OAuth
 *  guide). Where a disconnect that kept PM's access sends you to finish the job by hand. */
export const GOOGLE_PERMISSIONS_URL = "https://myaccount.google.com/permissions";

/** One Google disconnect's outcome: which service left which account, and what kept PM's access. */
export interface GoogleGrantOutcome {
  service: "calendar" | "drive";
  email: string;
  keptFor: GoogleUse[];
}

const USE_NAME: Record<GoogleUse, string> = {
  calendar: "Google Calendar",
  drive: "Google Drive",
  backup: "backups",
};

/** "a", "a and b", "a, b and c". */
function joinNames(names: string[]): string {
  if (names.length <= 1) return names.join("");
  return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
}

/**
 * The line shown after a Google disconnect that kept PM's access alive, or `null` when nothing kept
 * it (the disconnect revoked it). Google revokes an app's access to the whole account, not one
 * service's share of it, so while another PM feature still signs in as the account PM leaves the
 * access in place and says so — otherwise the disconnect would read as not having worked. Ends where
 * the caller appends the link to {@link GOOGLE_PERMISSIONS_URL}.
 *
 * Backups are named differently on purpose: turning them off never revokes (a re-granted `drive.file`
 * couldn't manage the backups the old grant made, #600), so "disconnect that too" would be untrue.
 */
export function googleGrantNote(outcome: GoogleGrantOutcome): string | null {
  const { service, email, keptFor } = outcome;
  if (keptFor.length === 0) return null;
  const lead = `Disconnected ${email} from ${USE_NAME[service]}. PM still uses this account for ${joinNames(keptFor.map((u) => USE_NAME[u]))}, so Google keeps PM's access to it.`;
  const others = keptFor.filter((u) => u !== "backup").map((u) => USE_NAME[u]);
  if (!keptFor.includes("backup")) {
    return `${lead} To remove PM's access completely, disconnect ${others.length === 1 ? "that" : "those"} too, or remove PM at`;
  }
  const disconnectOthers = others.length > 0 ? `disconnect ${joinNames(others)} and ` : "";
  return `${lead} Turning off backups keeps that access too, so PM can still tidy the backups it already made. To remove it completely, ${disconnectOthers}remove PM at`;
}
