// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// How the Focus tab orders its project list. Pure and dependency-free so the ordering — the thing
// that decides what you look at first — is unit-tested rather than inferred from a component.
//
// "Smart" is the default and the only composite key: it ranks by DUE SOON, then PRIORITY, then LAST
// ACTIVE. It used to be the status ladder alone, which meant priority was never consulted under
// Smart at all (importance was a separate, mutually exclusive key) and, worse, projects inside the
// due-soon bucket were ordered ALPHABETICALLY — so something due today could sit below something due
// next week. Every input the new ordering needs is already on the payload; nothing was added to the
// backend for it.
//
// The other keys stay exactly as they were: one explicit attribute, in either direction.

import { rankImportance } from "./importance";
import type { Importance, ProjectOverview } from "./types";

/** The backend's status precedence. Still used by the non-Smart display and kept as a value here so
 *  nothing has to re-derive it; Smart no longer sorts on it directly. */
export const STATUS_ORDER = [
  "due_soon",
  "blocked",
  "quick_win",
  "take_a_look",
  "part_of",
  "on_track",
] as const;

export type SortKey = "smart" | "deadline" | "importance" | "size" | "recent";

export interface Sort {
  key: SortKey;
  dir: "asc" | "desc";
}

export const SORT_LABELS: Record<SortKey, string> = {
  smart: "Smart",
  deadline: "Deadline",
  importance: "Importance",
  size: "Size",
  recent: "Recent active",
};

/** The natural direction for each key when it's first chosen (the ↑/↓ toggle flips it). */
export const DEFAULT_DIR: Record<SortKey, "asc" | "desc"> = {
  smart: "asc", // most pressing first
  deadline: "asc", // soonest first
  importance: "desc", // highest first
  size: "desc", // largest first
  recent: "desc", // most recently active first
};

const SIZE_RANK: Record<string, number> = { quick: 1, standard: 2, large: 3 };

export const SORT_LS_KEY = "pm.focus.sort";

/** The date a deadline-sort ranks on: the governing milestone, else a name-matched calendar event,
 *  else a far-future sentinel so undated projects sort last (ascending). */
export function deadlineKey(p: ProjectOverview): string {
  return (p.governing_milestone?.due_date ?? p.calendar_event?.start ?? "9999-12-31").slice(0, 10);
}

/** The priority a project shows: the manual override, falling back to the computed structural
 *  auto-importance (the "Auto" value) when no override is set — the same value the card displays. */
export function effectiveImportance(p: ProjectOverview): Importance {
  return p.importance ?? p.auto_importance;
}

/** Ascending comparison for one sort key (the ↑/↓ toggle applies the direction outside). */
export function ascCompare(a: ProjectOverview, b: ProjectOverview, key: SortKey): number {
  switch (key) {
    case "smart": {
      // 1. Due soon first. Reuse the BACKEND's own window (deadline within 7 days, or overdue,
      //    calendar fallback included) rather than re-deriving it here — and inside that tier order
      //    by the actual date, which is what was missing.
      const da = a.status === "due_soon";
      const db = b.status === "due_soon";
      if (da !== db) return da ? -1 : 1;
      if (da && db) {
        const c = deadlineKey(a).localeCompare(deadlineKey(b));
        if (c) return c;
      }
      // 2. Priority, highest first. Written b-vs-a because the caller multiplies the result by the
      //    direction factor, so "ascending" has to still mean "most pressing first".
      const p = rankImportance(effectiveImportance(b)) - rankImportance(effectiveImportance(a));
      if (p) return p;
      // 3. Last active, most recent first; a project that has never been touched sinks.
      return (b.last_activity ?? "").localeCompare(a.last_activity ?? "");
    }
    case "deadline":
      return deadlineKey(a).localeCompare(deadlineKey(b));
    case "importance":
      return rankImportance(effectiveImportance(a)) - rankImportance(effectiveImportance(b));
    case "size":
      return (SIZE_RANK[a.size ?? ""] ?? 0) - (SIZE_RANK[b.size ?? ""] ?? 0);
    case "recent":
      return (a.last_activity ?? "").localeCompare(b.last_activity ?? "");
  }
}

/** The full ordering: the key's comparison, the direction, then name as a stable tiebreak. */
export function compareProjects(a: ProjectOverview, b: ProjectOverview, sort: Sort): number {
  const factor = sort.dir === "asc" ? 1 : -1;
  return ascCompare(a, b, sort.key) * factor || a.name.localeCompare(b.name);
}

/** A sorted copy. */
export function sortProjects(projects: ProjectOverview[], sort: Sort): ProjectOverview[] {
  return [...projects].sort((a, b) => compareProjects(a, b, sort));
}

export function readSort(): Sort {
  try {
    const raw = localStorage.getItem(SORT_LS_KEY);
    if (raw) {
      const s = JSON.parse(raw);
      if (s && s.key in SORT_LABELS && (s.dir === "asc" || s.dir === "desc")) return s;
    }
  } catch {
    /* fall through to the default */
  }
  return { key: "smart", dir: "asc" };
}

export function writeSort(sort: Sort): void {
  try {
    localStorage.setItem(SORT_LS_KEY, JSON.stringify(sort));
  } catch {
    /* best-effort */
  }
}
