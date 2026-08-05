// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Which columns the Documents table shows (#701).
//
// **Depth seeds the default; the picker is the control.** Before this, Depth WAS the control — it
// alone decided whether the Ingested column appeared. Adding source author / modified-by / created /
// size would have made that a nine-column `table-fixed` layout at Power, squeezing Title on any
// narrow window, and "what do I want to see in this table" is not the same question as "how much
// detail do I want across the whole app".
//
// So the picker takes over: Depth still supplies the starting set, and a Reset puts it back. That
// keeps one control for one thing rather than growing a second preference beside the one that
// already governed it — the picker replaces Depth's authority over this table instead of competing
// with it. A user who never opens the menu sees exactly what they saw before, at every Depth.
//
// localStorage rather than a backend Setting, by the same rule as the rest of `documentPrefs`: which
// columns are on is a statement about this person at this machine, not about the library.

import type { Depth } from "../theme";
import { SOURCE_FACT_KEYS, SOURCE_FACT_LABELS, type SourceFactKey } from "./sourceFacts";

/** Every column the table can render, in display order. `title` is not here: it is always shown and
 *  is not offered in the picker, because a table of documents with no titles is not a table. */
export const DOC_COLUMN_KEYS = [
  "project",
  "importance",
  "source",
  "chunks",
  ...SOURCE_FACT_KEYS,
  "ingested",
  "synced",
] as const;
export type DocColumnKey = (typeof DOC_COLUMN_KEYS)[number];

/** The picker's label for each column — the source facts reuse their own wording so the table and
 *  the compare cards can never disagree about what "Modified by" means. */
export const DOC_COLUMN_LABELS: Record<DocColumnKey, string> = {
  project: "Project",
  importance: "Importance",
  source: "Source",
  chunks: "Chunks",
  ...SOURCE_FACT_LABELS,
  ingested: "Ingested",
  synced: "Last synced",
};

/** How wide a column is allowed to get, for the columns that hold free text.
 *
 *  **The table sizes columns to their contents; these are ceilings, not widths.** It used to be
 *  `table-fixed` with a fixed width per column and Title taking the leftover, which meant every
 *  column was as wide as its worst case whether or not anything in it was that long — an Importance
 *  column ten characters wide holding "high", a Chunks column holding "7" — and the slack all
 *  collected in the Title cell as one visible hole.
 *
 *  Only free text needs a ceiling: an author's name or an absolute path has no natural width and one
 *  long value in one row would otherwise set the width of the whole column. The fixed-format columns
 *  (the dates, the sizes, the counts) are deliberately absent — they are self-limiting, so a ceiling
 *  on them could only ever truncate a value that was going to fit anyway.
 *
 *  These are applied to an inner block, not to the cell: `truncate` needs a bounded box to put its
 *  ellipsis in, and an `overflow: hidden` block is also what lets the column shrink below its text
 *  when the window is narrow instead of pushing the table sideways. */
export const DOC_COLUMN_CAPS: Partial<Record<DocColumnKey, string>> = {
  project: "max-w-[12rem]",
  source: "max-w-[15rem]",
  author: "max-w-[11rem]",
  modifiedBy: "max-w-[11rem]",
};

/** The ceiling on the title itself, and on the location line under it. Title still takes the
 *  leftover width, so this only bites on a narrow window — it is here so the two lines of the Title
 *  cell agree with each other rather than one of them setting the column's width. */
export const DOC_TITLE_CAP = "max-w-[38rem]";

/** Whether a column is a source fact (rendered from `sourceFacts`) rather than a PM-side field. */
export function isSourceFactColumn(key: DocColumnKey): key is SourceFactKey {
  return (SOURCE_FACT_KEYS as readonly string[]).includes(key);
}

/** The set a given Depth starts from.
 *
 *  Chosen by hand rather than derived: `min` earns its name by showing the one thing that answers
 *  "where does this live", `standard` adds the two facts people actually scan a library by (who
 *  wrote it, how big it is) plus how much it matters, and `power` adds the chunk count, which is a
 *  statement about the index rather than about the document.
 *
 *  Everything else — who last changed it, when it was created, when it was updated, when PM ingested
 *  or last synced it — is off until asked for. They are real questions, but they are questions about
 *  ONE document, and a column answers them for a thousand at the cost of the width the title needs.
 *
 *  This replaces the original seeding, which was "exactly what the table showed before the picker
 *  existed". That was the right call while the picker was new and nobody had opinions about it yet;
 *  it is not a permanent claim. Note who moves: a user who has never opened the picker follows this
 *  and will see their table change shape once (at Power, `Ingested` goes and `Author`/`Size` arrive);
 *  a user who has ever toggled a column has a stored explicit set and is untouched until they press
 *  Reset. */
export function defaultColumns(depth: Depth): DocColumnKey[] {
  const wanted: DocColumnKey[] =
    depth === "min"
      ? ["project"]
      : depth === "standard"
        ? ["project", "importance", "author", "size"]
        : ["project", "importance", "chunks", "author", "size"];
  // Filtered through the canonical order rather than trusted as written, so the lists above can be
  // read as sets and a reordering here can never desync from the header order.
  return DOC_COLUMN_KEYS.filter((k) => wanted.includes(k));
}

export const DOC_COLUMNS_KEY = "pm.documents.columns";

/** The user's stored column choice, or `null` when they have never made one.
 *
 *  `null` rather than a default, so the caller keeps ownership of the Depth seed — and so switching
 *  Depth keeps working for anyone who has not touched the picker. Unknown keys are dropped rather
 *  than failing the read: a column removed in a later version must not strand the whole preference
 *  (the `oneOf`-coercion trap from PR #538, one layer up). An empty result after filtering is
 *  treated as no choice at all, since a table with only titles is not something anyone chose. */
export function readColumns(): DocColumnKey[] | null {
  try {
    const raw = localStorage.getItem(DOC_COLUMNS_KEY);
    if (raw == null) return null;
    const parsed: unknown = JSON.parse(raw);
    if (!Array.isArray(parsed)) return null;
    const known = DOC_COLUMN_KEYS.filter((k) => parsed.includes(k));
    return known.length > 0 ? [...known] : null;
  } catch {
    return null;
  }
}

/** Persist a column choice. Passing `null` clears it, so the table goes back to following Depth —
 *  which is what Reset does, rather than writing the current Depth's set out as an explicit choice
 *  that would then stop following Depth. */
export function writeColumns(columns: DocColumnKey[] | null): void {
  try {
    if (columns == null) {
      localStorage.removeItem(DOC_COLUMNS_KEY);
    } else {
      // Stored in canonical display order, so the table never has to sort it and two writes of the
      // same set compare equal.
      const ordered = DOC_COLUMN_KEYS.filter((k) => columns.includes(k));
      localStorage.setItem(DOC_COLUMNS_KEY, JSON.stringify(ordered));
    }
  } catch {
    /* best-effort — a private-mode / quota failure just means the choice won't persist */
  }
}

/** Toggle one column within a set, keeping canonical order. Turning the last one off is allowed:
 *  Title still renders, and the picker's Reset is always there. */
export function toggleColumn(columns: DocColumnKey[], key: DocColumnKey): DocColumnKey[] {
  const next = columns.includes(key) ? columns.filter((k) => k !== key) : [...columns, key];
  return DOC_COLUMN_KEYS.filter((k) => next.includes(k));
}
