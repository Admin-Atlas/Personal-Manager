// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Which free-form tags look like the same tag (#579).
//
// A tag earns its keep by GROUPING documents, so `tax` with two files and `taxes` with three is
// worse than useless — it is one group of five pretending to be two groups of nothing. The filing
// prompt now steers away from coining near-duplicates (#578) and the re-tag pass picks from a closed
// vocabulary (#580), but neither repairs what a store already accumulated.
//
// This is deliberately the SAME shape as `findIdenticalPairs` for projects in TeachView: high
// precision, user-confirmed, no model. Two rules only, both of which a person would agree with
// instantly on seeing the pair:
//
//   1. identical once case, spacing and punctuation are ignored — `meeting-notes` / `meeting notes`;
//   2. one is the other's simple plural — `tax` / `taxes`, `chair` / `chairs`, `policy` / `policies`.
//
// Deliberately NOT edit distance, which pairs `tax` with `fax` and `cv` with `cs`, and not
// embeddings, which would make an offline, instant, explainable check into none of those. A missed
// pair costs nothing — the user can still rename either tag by hand. A WRONG pair offered as a
// one-click fold costs real data, because folding is a bulk vault rewrite with no undo.

import type { TagSummary } from "./types";

/** Case, spacing and punctuation removed — the key the "these are the same" rule compares on. */
export function foldKey(name: string): string {
  return name.toLowerCase().replace(/[^a-z0-9]/g, "");
}

/** Is `b` `a`'s simple English plural? Checked on the folded key, so `meeting-note`/`meeting notes`
 *  counts too. */
function isPluralOf(a: string, b: string): boolean {
  if (a === b) return false;
  // policy → policies
  if (a.endsWith("y") && b === `${a.slice(0, -1)}ies`) return true;
  // box → boxes, class → classes
  if (b === `${a}es`) return true;
  // tax → taxs is not English, but `invoice` → `invoices` is the common case.
  return b === `${a}s`;
}

/** True when two tag names are near-duplicates by either rule, in either direction. */
export function looksLikeSameTag(a: string, b: string): boolean {
  const [x, y] = [foldKey(a), foldKey(b)];
  if (!x || !y) return false;
  if (x === y) return true;
  return isPluralOf(x, y) || isPluralOf(y, x);
}

/**
 * Pairs of tags that look like the same tag, best-survivor first.
 *
 * The first of each pair is the one carrying MORE documents — the one a fold should keep, since
 * folding into the rarer spelling would rewrite more files to reach the same place. Ties break
 * alphabetically so the list never reshuffles between renders.
 *
 * Each tag appears in at most one pair. Three spellings of one word (`tax`, `taxes`, `taxation`)
 * surface as one pair now and the next after that fold, rather than as a tangle of overlapping
 * suggestions where accepting one silently invalidates another.
 */
export function findSimilarTags(tags: readonly TagSummary[]): Array<[TagSummary, TagSummary]> {
  const groups = tags.filter((t) => t.kind !== "project");
  const used = new Set<string>();
  const out: Array<[TagSummary, TagSummary]> = [];

  for (let i = 0; i < groups.length; i++) {
    if (used.has(groups[i].name)) continue;
    for (let j = i + 1; j < groups.length; j++) {
      if (used.has(groups[j].name)) continue;
      if (!looksLikeSameTag(groups[i].name, groups[j].name)) continue;
      const pair: [TagSummary, TagSummary] =
        groups[j].documents > groups[i].documents
          ? [groups[j], groups[i]]
          : groups[i].documents > groups[j].documents
            ? [groups[i], groups[j]]
            : ([groups[i], groups[j]].sort((a, b) => a.name.localeCompare(b.name)) as [
                TagSummary,
                TagSummary,
              ]);
      used.add(pair[0].name);
      used.add(pair[1].name);
      out.push(pair);
      break;
    }
  }
  return out;
}
