// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

/**
 * The folder picker's tri-state scope: which folders are explicitly indexed (`selected` — the walk's
 * seed roots) and which subfolders are skipped within an indexed subtree (`excluded`). A folder is
 * effectively indexed when it is selected, or sits under a selected root (or the whole-folder root, for
 * local folders), AND neither it nor any ancestor is excluded.
 */
export interface FolderSelection {
  selected: string[];
  excluded: string[];
}

const without = (a: string[], x: string) => a.filter((v) => v !== x);
const withId = (a: string[], x: string) => (a.includes(x) ? a : [...a, x]);

/**
 * Apply one checkbox toggle and return the next `{selected, excluded}`. `ancestorIncluded` means a
 * parent folder is already effectively indexed (true at the root for local folders, where the whole
 * folder is indexed); `want` is the checkbox's new state.
 *
 * - Checking drops any exclude on the folder, and — only if it isn't already covered by an included
 *   ancestor — adds it as a new seed root.
 * - Unchecking removes it from the seed roots if it was one, and, when it would still be indexed via an
 *   ancestor, adds an exclude so the subtree is pruned.
 *
 * Toggles are never offered for a folder under an excluded ancestor (the picker disables those), so the
 * two levers never fight.
 */
export function applyFolderToggle(
  cur: FolderSelection,
  id: string,
  ancestorIncluded: boolean,
  want: boolean,
): FolderSelection {
  if (want) {
    return {
      selected: ancestorIncluded ? cur.selected : withId(cur.selected, id),
      excluded: without(cur.excluded, id),
    };
  }
  let selected = cur.selected;
  let excluded = cur.excluded;
  if (selected.includes(id)) selected = without(selected, id);
  if (ancestorIncluded) excluded = withId(excluded, id);
  return { selected, excluded };
}

/**
 * Drop a folder from `selected` when it has become a **redundant or stranded seed root** — it is in
 * `selected` yet sits under an included ancestor (redundant: the ancestor already covers it) or under
 * an excluded ancestor (stranded: the user excluded a folder above it). Both are invisible in the UI
 * (the checkbox shows the ancestor's state), and a stranded seed would otherwise make the backend index
 * a subtree the user excluded, since the walk seeds each `selected` root independently. Returns `cur`
 * unchanged when there's nothing to prune, so callers can compare by identity. `null` id is a no-op.
 */
export function pruneStrandedSeed(
  cur: FolderSelection,
  id: string,
  ancestorIncluded: boolean,
  ancestorExcluded: boolean,
): FolderSelection {
  if (!cur.selected.includes(id) || !(ancestorIncluded || ancestorExcluded)) {
    return cur;
  }
  return { selected: without(cur.selected, id), excluded: cur.excluded };
}
