// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Per-device sort + "show completed" prefs for a project's Milestones list, remembered across every
// project (one shared choice, like the Focus project sort). Display-only — the backend order
// (sort_order) is untouched, so governing()/status derivation is unaffected — so these live in
// localStorage, never a backend Setting (mirrors mapPrefs / the Focus sort).

export type MsSortKey = "manual" | "deadline" | "label";
export interface MsSort {
  key: MsSortKey;
  dir: "asc" | "desc";
}

const SORT_KEY = "pm.milestones.sort";
const SHOW_COMPLETED_KEY = "pm.milestones.showCompleted";
const SORT_KEYS: readonly MsSortKey[] = ["manual", "deadline", "label"];

/** The stored sort; defaults to deadline ascending (soonest first) when absent or invalid. */
export function readMilestoneSort(): MsSort {
  try {
    const raw = localStorage.getItem(SORT_KEY);
    if (raw) {
      const s = JSON.parse(raw);
      if (s && SORT_KEYS.includes(s.key) && (s.dir === "asc" || s.dir === "desc")) return s;
    }
  } catch {
    /* fall through to the default */
  }
  return { key: "deadline", dir: "asc" };
}

export function writeMilestoneSort(sort: MsSort): void {
  try {
    localStorage.setItem(SORT_KEY, JSON.stringify(sort));
  } catch {
    /* best-effort — a private-mode / quota failure just means it won't persist */
  }
}

/** Whether completed ("met") milestones are shown; defaults to true (don't hide history on open). */
export function readShowCompletedMilestones(): boolean {
  try {
    return localStorage.getItem(SHOW_COMPLETED_KEY) !== "false";
  } catch {
    return true;
  }
}

export function writeShowCompletedMilestones(show: boolean): void {
  try {
    localStorage.setItem(SHOW_COMPLETED_KEY, show ? "true" : "false");
  } catch {
    /* best-effort */
  }
}
