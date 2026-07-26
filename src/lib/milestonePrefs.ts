// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Per-device sort + "show completed" prefs for a project's Milestones list. Display-only — the
// backend order (sort_order) is untouched, so governing()/status derivation is unaffected — so these
// live in localStorage, never a backend Setting (mirrors mapPrefs / the Focus sort).
//
// The SORT is per project. It used to be one shared choice across every project, which is wrong for
// this control in a way it isn't for the Focus project sort: projects differ in whether they have
// dates at all, and a hand-ordered plan and a deadline-driven one want opposite orders. Stored as a
// map under the SAME key; a pre-existing bare {key,dir} is still read and becomes the fallback for
// projects that have no choice of their own, so nobody's setting is discarded on upgrade.
//
// "Show completed" stays global on purpose — it is a visibility habit, not a property of a plan.

export type MsSortKey = "manual" | "deadline" | "label";
export interface MsSort {
  key: MsSortKey;
  dir: "asc" | "desc";
}

const SORT_KEY = "pm.milestones.sort";
const SHOW_COMPLETED_KEY = "pm.milestones.showCompleted";
const SORT_KEYS: readonly MsSortKey[] = ["manual", "deadline", "label"];

const DEFAULT_SORT: MsSort = { key: "deadline", dir: "asc" };

function isSort(v: unknown): v is MsSort {
  const s = v as MsSort | null;
  return !!s && SORT_KEYS.includes(s.key) && (s.dir === "asc" || s.dir === "desc");
}

/** The whole stored map, plus any legacy single value found in its place. */
function readSortStore(): { map: Record<string, MsSort>; legacy: MsSort | null } {
  try {
    const raw = localStorage.getItem(SORT_KEY);
    if (raw) {
      const parsed: unknown = JSON.parse(raw);
      // A bare {key,dir} is the pre-upgrade shape — one choice that applied everywhere.
      if (isSort(parsed)) return { map: {}, legacy: parsed };
      if (parsed && typeof parsed === "object") {
        const map: Record<string, MsSort> = {};
        for (const [name, v] of Object.entries(parsed as Record<string, unknown>)) {
          if (isSort(v)) map[name] = v;
        }
        return { map, legacy: null };
      }
    }
  } catch {
    /* fall through to the defaults */
  }
  return { map: {}, legacy: null };
}

/** This project's sort: its own choice, else the pre-upgrade shared one, else deadline ascending. */
export function readMilestoneSort(project: string): MsSort {
  const { map, legacy } = readSortStore();
  return map[project] ?? legacy ?? DEFAULT_SORT;
}

export function writeMilestoneSort(project: string, sort: MsSort): void {
  try {
    // The first write replaces the legacy bare value with a map. That is intended: from then on a
    // project without its own entry falls back to the DEFAULT rather than to the old shared choice,
    // which is what "per project" means. Every project chosen before then keeps its entry.
    const { map } = readSortStore();
    localStorage.setItem(SORT_KEY, JSON.stringify({ ...map, [project]: sort }));
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
