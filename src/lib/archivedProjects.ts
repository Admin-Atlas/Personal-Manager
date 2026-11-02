// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Archived projects (#1018), as the surfaces that list things BY PROJECT NAME see them — the Chats
// sidebar and the Documents list. Focus doesn't need this: its overviews carry `archived` directly.
//
// Names compare trimmed and case-folded, the way the backend's membership roster does
// (`lower(trim(name))`), so a document filed under "atlas " still counts as in an archived "Atlas".

export type ArchivedSet = ReadonlySet<string>;

const norm = (name: string) => name.trim().toLowerCase();

export function archivedSet(names: readonly string[]): ArchivedSet {
  return new Set(names.map(norm));
}

export function isArchived(set: ArchivedSet, name: string | null | undefined): boolean {
  return name != null && set.has(norm(name));
}

/** Split documents by whether their HOME project is archived. Only the home counts: a file merely
 *  linked to an archived project also belongs to a live one, and hiding it would hide live work. */
export function splitByArchivedHome<T extends { project: string }>(
  docs: readonly T[],
  set: ArchivedSet,
): { shown: T[]; hidden: T[] } {
  if (set.size === 0) return { shown: [...docs], hidden: [] };
  const shown: T[] = [];
  const hidden: T[] = [];
  for (const d of docs) (isArchived(set, d.project) ? hidden : shown).push(d);
  return { shown, hidden };
}
