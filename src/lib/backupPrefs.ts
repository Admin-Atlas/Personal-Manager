// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Persistence for the backup panel's one-time "you have more backups than your keep-last-N"
// reconciliation banner. Dismissal is remembered per destination + account, so switching to a
// different Proton/Google account (a genuinely different backup location) surfaces the banner again
// rather than staying hidden by a prior account's dismissal.
//
// The ACCOUNT is a cloud identifier — an email address — and it used to sit in the localStorage KEY
// NAME, one dynamically-named plaintext key per account ever connected, which nothing enumerated and
// nothing pruned. The dismissals now live in the encrypted `settings` table under `backup_ui` as one
// flat list of `<destination>.<account>` ids (see storedPrefs.ts), so they are encrypted, findable,
// travel in a `.pmbackup`, and go away with PM's data.

import { readStored, writeStored } from "./storedPrefs";

const PREF_KEY = "backup_ui";

/** The stored id. `account` may be absent and may itself contain dots (it is an email address), so
 *  this composite is BUILT the same way on both sides and never split back apart. */
const id = (destination: string, account: string | null) => `${destination}.${account ?? ""}`;

function dismissed(): string[] {
  const arr = readStored(PREF_KEY).reconcileDismissed;
  return Array.isArray(arr) ? arr.filter((x): x is string => typeof x === "string") : [];
}

/** Whether the reconciliation banner for this destination + account was dismissed. */
export function readReconcileDismissed(destination: string, account: string | null): boolean {
  return dismissed().includes(id(destination, account));
}

export function writeReconcileDismissed(destination: string, account: string | null): void {
  const key = id(destination, account);
  const current = dismissed();
  if (current.includes(key)) return;
  writeStored(PREF_KEY, { reconcileDismissed: [...current, key] });
}
