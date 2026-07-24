// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Local, best-effort persistence for the backup panel's one-time "you have more backups than your
// keep-last-N" reconciliation banner. Dismissal is remembered per destination + account, so
// switching to a different Proton/Google account (a genuinely different backup location) surfaces
// the banner again rather than staying hidden by a prior account's dismissal. Mirrors the
// try/catch localStorage shape of reviewPrefs.ts / milestonePrefs.ts (a private-mode or quota
// failure just means it won't persist — never throws into the UI).

const key = (destination: string, account: string | null) =>
  `pm.backup.reconcileDismissed.${destination}.${account ?? ""}`;

/** Whether the reconciliation banner for this destination + account was dismissed. */
export function readReconcileDismissed(destination: string, account: string | null): boolean {
  try {
    return localStorage.getItem(key(destination, account)) === "true";
  } catch {
    return false;
  }
}

export function writeReconcileDismissed(destination: string, account: string | null): void {
  try {
    localStorage.setItem(key(destination, account), "true");
  } catch {
    /* best-effort — a private-mode / quota failure just means it won't persist */
  }
}
