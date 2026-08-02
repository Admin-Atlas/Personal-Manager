// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// What the SOURCE knows about a document — author, who last changed it, when it was created there,
// how big it is (#701).
//
// One module because three surfaces render the same four facts and must agree on every word of
// them: the duplicate compare cards (which is what prompted the ask — two copies of one file have to
// be tellable apart before anyone is asked to delete one), the Documents table, and the reader.
//
// **`null` means the provider did not say, and that renders as "Unknown".** Never blank, and never
// "you": a document's author reading as the person looking at it is worse than no answer, and a
// filesystem genuinely has no author to report. That decision is the reason this is a shared
// function rather than three `?? "—"` fallbacks that would drift apart.

import { formatBytes, formatDateOnly } from "./format";
import type { Document } from "./types";

/** What a provider that will not say is rendered as, everywhere. */
export const UNKNOWN = "Unknown";

/** Which source facts exist, in the order they read. Doubles as the column key in the Documents
 *  table, so a stored column choice and a rendered row can't drift apart. */
export const SOURCE_FACT_KEYS = ["author", "modifiedBy", "created", "size"] as const;
export type SourceFactKey = (typeof SOURCE_FACT_KEYS)[number];

/** The heading each fact is shown under — one wording for all three surfaces. */
export const SOURCE_FACT_LABELS: Record<SourceFactKey, string> = {
  author: "Author",
  modifiedBy: "Modified by",
  created: "Created",
  size: "Size",
};

/** One fact, already rendered. `known` is false when the value is the {@link UNKNOWN} placeholder,
 *  so a surface can dim it without re-deriving which fields were null. */
export interface SourceFact {
  key: SourceFactKey;
  label: string;
  value: string;
  known: boolean;
}

/** The rendered value of one fact, or {@link UNKNOWN}. Dates go through `formatDateOnly`, so they
 *  read DD-MM-YYYY like every other date in the app (and drop the year inside the current one). */
export function sourceFactValue(doc: Document, key: SourceFactKey): string {
  switch (key) {
    case "author":
      return doc.source_author ?? UNKNOWN;
    case "modifiedBy":
      return doc.source_last_modified_by ?? UNKNOWN;
    case "created":
      return doc.source_created_at ? formatDateOnly(doc.source_created_at) : UNKNOWN;
    case "size":
      // `formatBytes` renders null as "—"; this surface says Unknown instead, because a missing size
      // means the same thing here as a missing author — the source didn't say. A Google-native Doc
      // has no byte size at all, which is exactly that case.
      return doc.source_size_bytes == null ? UNKNOWN : formatBytes(doc.source_size_bytes);
  }
}

/** Whether the source actually reported this fact. */
export function sourceFactKnown(doc: Document, key: SourceFactKey): boolean {
  switch (key) {
    case "author":
      return doc.source_author != null;
    case "modifiedBy":
      return doc.source_last_modified_by != null;
    case "created":
      return doc.source_created_at != null;
    case "size":
      return doc.source_size_bytes != null;
  }
}

/** All four facts, always — an absent one reads "Unknown" rather than vanishing.
 *
 *  Fixed-length on purpose: on the compare cards the two sides sit next to each other, and a card
 *  that dropped its unknown rows would put "Created" on one side level with "Size" on the other. */
export function sourceFacts(doc: Document): SourceFact[] {
  return SOURCE_FACT_KEYS.map((key) => ({
    key,
    label: SOURCE_FACT_LABELS[key],
    value: sourceFactValue(doc, key),
    known: sourceFactKnown(doc, key),
  }));
}
