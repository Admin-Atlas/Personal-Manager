// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Sort + "show completed" prefs for a project's Milestones list. Display-only — the backend order
// (sort_order) is untouched, so governing()/status derivation is unaffected.
//
// The SORT MAP IS KEYED BY PROJECT NAME, so the store itself is user content ("Taxes 2025",
// "Divorce", a client's name). It therefore lives in the encrypted `settings` table under
// `milestone_ui` rather than in the webview's plaintext localStorage — see storedPrefs.ts, which
// keeps these reads synchronous and carried the pre-upgrade localStorage copy across once.
//
// The SORT is per project. It used to be one shared choice across every project, which is wrong for
// this control in a way it isn't for the Focus project sort: projects differ in whether they have
// dates at all, and a hand-ordered plan and a deadline-driven one want opposite orders. Stored as a
// map under the SAME field; a pre-existing bare {key,dir} is still read and becomes the fallback for
// projects that have no choice of their own, so nobody's setting is discarded on upgrade.
//
// "Show completed" stays global on purpose — it is a visibility habit, not a property of a plan. It
// rides in the same blob (one home per module) and so survives a backup+restore too.

import { readStored, writeStored } from "./storedPrefs";

export type MsSortKey = "manual" | "deadline" | "label";
export interface MsSort {
  key: MsSortKey;
  dir: "asc" | "desc";
}

const PREF_KEY = "milestone_ui";
const SORT_KEYS: readonly MsSortKey[] = ["manual", "deadline", "label"];

const DEFAULT_SORT: MsSort = { key: "deadline", dir: "asc" };

function isSort(v: unknown): v is MsSort {
  const s = v as MsSort | null;
  return !!s && SORT_KEYS.includes(s.key) && (s.dir === "asc" || s.dir === "desc");
}

/** The whole stored map, plus any legacy single value found in its place. */
function readSortStore(): { map: Record<string, MsSort>; legacy: MsSort | null } {
  try {
    const parsed = readStored(PREF_KEY).sort;
    // A bare {key,dir} is the pre-upgrade shape — one choice that applied everywhere. It can still
    // arrive here because hydration carries the localStorage value across verbatim.
    if (isSort(parsed)) return { map: {}, legacy: parsed };
    if (parsed && typeof parsed === "object") {
      const map: Record<string, MsSort> = {};
      for (const [name, v] of Object.entries(parsed as Record<string, unknown>)) {
        if (isSort(v)) map[name] = v;
      }
      return { map, legacy: null };
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
  // The first write replaces the legacy bare value with a map. That is intended: from then on a
  // project without its own entry falls back to the DEFAULT rather than to the old shared choice,
  // which is what "per project" means. Every project chosen before then keeps its entry.
  const { map } = readSortStore();
  writeStored(PREF_KEY, { sort: { ...map, [project]: sort } });
}

/** Whether completed ("met") milestones are shown; defaults to true (don't hide history on open). */
export function readShowCompletedMilestones(): boolean {
  return readStored(PREF_KEY).showCompleted !== false;
}

export function writeShowCompletedMilestones(show: boolean): void {
  writeStored(PREF_KEY, { showCompleted: show });
}
