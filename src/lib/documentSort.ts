// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// How the Documents table is ordered, and the fact that the ordering OUTLIVES the table.
//
// The sort used to be a `useState` in DocumentsView, which meant it died on every tab switch: the
// Documents tab is a branch of App's view ternary inside an `ErrorBoundary` keyed on the view, so
// leaving the tab unmounts the whole thing. You sorted by size, went to look at something, came
// back, and were reading newest-first again with no arrow anywhere to say why. That is the same
// failure `documentPrefs` was written for, one control further along.
//
// localStorage rather than a backend Setting, by the same rule as the rest of `pm.documents.*`: a
// sort order is a statement about this person at this machine, not about the library. It is also not
// user CONTENT — it is a pair of enums — so it has no business travelling inside an encrypted vault
// or a `.pmbackup`.
//
// The comparator lives here rather than in the component for the same reason `focusSort` does: "what
// order do I look at things in" is worth testing directly rather than inferring from a rendered
// table.

import { DOC_COLUMN_KEYS, type DocColumnKey } from "./documentColumns";
import { rankImportance } from "./importance";
import { sourceRank } from "./sourceLabel";
import type { Document } from "./types";

/** Every column sorts, plus the always-present title. A header only sorts when it is rendered, so
 *  which keys are reachable by clicking follows the column picker — but a STORED key can name a
 *  column that is currently hidden, which is handled at the call site rather than here. */
export type SortKey = "title" | DocColumnKey;

export interface DocSort {
  key: SortKey;
  dir: "asc" | "desc";
}

/** Columns where "biggest / most recent first" is the more useful first click. */
const SORT_DESC_FIRST = new Set<SortKey>([
  "importance",
  "chunks",
  "created",
  "updated",
  "size",
  "ingested",
  "synced",
]);

/** Whether a value is a key this build can sort by. Exported because the table has to answer the
 *  same question about a RESTORED sort whose column may since have been switched off. */
export function isSortKey(value: unknown): value is SortKey {
  return value === "title" || (DOC_COLUMN_KEYS as readonly unknown[]).includes(value);
}

/** The sort that clicking `key` produces, given the current one: the same header again flips the
 *  direction, a new one starts in its natural direction. */
export function nextDocSort(current: DocSort | null, key: SortKey): DocSort {
  return current?.key === key
    ? { key, dir: current.dir === "asc" ? "desc" : "asc" }
    : { key, dir: SORT_DESC_FIRST.has(key) ? "desc" : "asc" };
}

/**
 * Where two rows sit relative to each other when one of them has no value at all: the absent one
 * goes LAST, whichever way the column is sorted. `null` means the pair needs an ordinary comparison
 * — either both have a value, or neither does, in which case the caller's title tiebreak decides.
 *
 * **The returned number is deliberately NOT multiplied by the direction factor.** The caller
 * early-returns it, so it is already the comparator's final answer; multiplying it — which is what
 * this did until the sort moved into this module — sent every unknown to the TOP the moment the
 * arrow flipped. That is the exact thing #707 set out to prevent when it made the source facts
 * sortable at all ("unknown values sort last in both directions, which answers the original
 * objection directly"), and what v3.123.2's release note promised, so the behaviour shipped
 * contradicting both. Ascending was right, which is why it went unnoticed.
 */
function unknownsLast<T>(a: T | null, b: T | null): number | null {
  if (a == null && b == null) return null;
  if (a == null) return 1;
  if (b == null) return -1;
  return null;
}

/**
 * Order `documents` by `sort`, without mutating the input. `null` means the backend's own order
 * (newest first), which is why it is returned untouched rather than sorted by some default.
 *
 * The source facts were originally display-only, on the reasoning that ordering by a column reading
 * "Unknown" for most rows just banks the Unknowns at one end. That is a real objection and it is
 * answered directly by `unknownsLast` rather than by refusing to sort: unknowns go LAST in both
 * directions, so clicking a header never buries the rows that have an answer.
 */
export function sortDocuments(documents: Document[], sort: DocSort | null): Document[] {
  if (!sort) return documents;
  const factor = sort.dir === "asc" ? 1 : -1;
  return [...documents].sort((a, b) => {
    let c = 0;
    switch (sort.key) {
      case "title":
        c = a.title.localeCompare(b.title);
        break;
      case "project":
        // The PRIMARY project: the column shows it, so it is what the header sorts by.
        c = a.project.localeCompare(b.project);
        break;
      case "importance":
        c = rankImportance(a.importance) - rankImportance(b.importance);
        break;
      case "source":
        // Never "unknown": every document is somewhere, so this needs no `unknownsLast` guard.
        c = sourceRank(a) - sourceRank(b);
        break;
      case "chunks":
        c = a.chunk_count - b.chunk_count;
        break;
      case "ingested":
        c = a.ingested_at.localeCompare(b.ingested_at);
        break;
      case "synced":
        // Never null in practice — the projection falls back to the ingest time — but typed as
        // nullable, so it goes through the same guard as the rest rather than being special-cased.
        {
          const u = unknownsLast(a.pm_refreshed_at, b.pm_refreshed_at);
          if (u !== null) return u;
          // Both absent reaches here and compares equal, falling through to the title tiebreak.
          c = (a.pm_refreshed_at ?? "").localeCompare(b.pm_refreshed_at ?? "");
        }
        break;
      case "author":
        {
          const u = unknownsLast(a.source_author, b.source_author);
          if (u !== null) return u;
          c = (a.source_author ?? "").localeCompare(b.source_author ?? "");
        }
        break;
      case "modifiedBy":
        {
          const u = unknownsLast(a.source_last_modified_by, b.source_last_modified_by);
          if (u !== null) return u;
          c = (a.source_last_modified_by ?? "").localeCompare(b.source_last_modified_by ?? "");
        }
        break;
      case "created":
        // ISO-8601 sorts lexicographically, which is why these compare as strings rather than
        // being parsed into Dates for every comparison of every pair.
        {
          const u = unknownsLast(a.source_created_at, b.source_created_at);
          if (u !== null) return u;
          c = (a.source_created_at ?? "").localeCompare(b.source_created_at ?? "");
        }
        break;
      case "updated":
        {
          const u = unknownsLast(a.source_modified_at, b.source_modified_at);
          if (u !== null) return u;
          c = (a.source_modified_at ?? "").localeCompare(b.source_modified_at ?? "");
        }
        break;
      case "size":
        {
          const u = unknownsLast(a.source_size_bytes, b.source_size_bytes);
          if (u !== null) return u;
          c = (a.source_size_bytes ?? 0) - (b.source_size_bytes ?? 0);
        }
        break;
    }
    if (c === 0) c = a.title.localeCompare(b.title); // stable tiebreak
    return c * factor;
  });
}

export const DOC_SORT_KEY = "pm.documents.sort";

/**
 * The stored sort, or `null` when there isn't a usable one.
 *
 * `null` rather than a default, so `null` keeps its one meaning throughout — the backend's own
 * newest-first order — and the caller keeps ownership of it. A stored key this build no longer has
 * (a column retired in a later version) reads as no sort at all rather than stranding the table in
 * an ordering nothing can explain: the `oneOf`-coercion trap from PR #538, answered the way
 * `readColumns` answers it.
 */
export function readDocSort(): DocSort | null {
  try {
    const raw = localStorage.getItem(DOC_SORT_KEY);
    if (raw == null) return null;
    const parsed: unknown = JSON.parse(raw);
    if (typeof parsed !== "object" || parsed == null) return null;
    const { key, dir } = parsed as { key?: unknown; dir?: unknown };
    if (!isSortKey(key)) return null;
    if (dir !== "asc" && dir !== "desc") return null;
    return { key, dir };
  } catch {
    return null;
  }
}

/** Persist a sort. Passing `null` clears it, so "back to newest first" and "never chose" are the
 *  same stored state — mirroring `writeColumns(null)`. */
export function writeDocSort(sort: DocSort | null): void {
  try {
    if (sort == null) localStorage.removeItem(DOC_SORT_KEY);
    else localStorage.setItem(DOC_SORT_KEY, JSON.stringify({ key: sort.key, dir: sort.dir }));
  } catch {
    /* best-effort — a private-mode / quota failure just means the sort won't persist */
  }
}
