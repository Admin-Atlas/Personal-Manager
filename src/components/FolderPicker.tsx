// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useState } from "react";
import { applyFolderToggle, pruneStrandedSeed, type FolderSelection } from "../lib/folderScope";

/** The minimal folder shape the picker renders — Drive, OneDrive and local folder rows all satisfy it. */
export interface PickerFolder {
  id: string;
  name: string;
}

interface LoadChildren {
  /** List the subfolders of `parentId`; `null` is the tree's root (the caller maps it to its own
   *  root sentinel). MUST be referentially stable (memoised on its inputs) — each level refetches
   *  when it changes. */
  (parentId: string | null): Promise<PickerFolder[]>;
}

/**
 * The shared lazy folder tree behind the Drive, OneDrive and local "choose folders" scope pickers: a
 * scrollable pane of checkbox rows, each with an expand caret that lazy-loads its subfolders.
 *
 * Selection is tri-state (see {@link FolderSelection}): a folder is checked when it is effectively
 * indexed — explicitly picked, or under a picked root (or `rootIncluded`, for local's whole-folder
 * case) — and not excluded. Unchecking a checked folder excludes its subtree; the checkboxes under an
 * excluded folder are disabled (un-exclude the parent first), so the include and exclude levers never
 * fight.
 */
export function FolderPicker({
  loadChildren,
  selected,
  excluded,
  rootIncluded = false,
  onChange,
  className,
}: {
  loadChildren: LoadChildren;
  selected: string[];
  excluded: string[];
  /** Whether the tree's root is itself indexed (true for a local folder: the whole folder is indexed,
   *  and the tree only chooses what to *exclude*). Drive/OneDrive leave it false — you pick folders in. */
  rootIncluded?: boolean;
  onChange: (next: FolderSelection) => void;
  /** Extra classes on the pane (the hosts differ only in top margin). */
  className?: string;
}) {
  return (
    <div
      className={`max-h-56 overflow-auto rounded-[var(--radius)] bg-surface p-1 ${className ?? ""}`}
    >
      <FolderChildren
        loadChildren={loadChildren}
        parentId={null}
        selected={selected}
        excluded={excluded}
        onChange={onChange}
        ancestorIncluded={rootIncluded}
        ancestorExcluded={false}
        depth={0}
      />
      <p className="px-1 pt-1 text-[0.625rem] text-ink4">
        {rootIncluded
          ? "The whole folder is indexed — uncheck a subfolder to skip it."
          : "Checking a folder indexes everything inside it; uncheck a subfolder to skip it."}
      </p>
    </div>
  );
}

/** One lazily-loaded level of the folder tree (children of `parentId`; `null` = the root). */
function FolderChildren({
  loadChildren,
  parentId,
  selected,
  excluded,
  onChange,
  ancestorIncluded,
  ancestorExcluded,
  depth,
}: {
  loadChildren: LoadChildren;
  parentId: string | null;
  selected: string[];
  excluded: string[];
  onChange: (next: FolderSelection) => void;
  ancestorIncluded: boolean;
  ancestorExcluded: boolean;
  depth: number;
}) {
  const [folders, setFolders] = useState<PickerFolder[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    loadChildren(parentId)
      .then((f) => active && setFolders(f))
      .catch((e) => active && setError(String(e)));
    return () => {
      active = false;
    };
  }, [loadChildren, parentId]);

  if (error) return <p className="px-2 py-1 text-xs text-st-due">{error}</p>;
  if (folders == null) return <p className="px-2 py-1 text-xs text-ink4">Loading…</p>;
  if (folders.length === 0)
    return depth === 0 ? <p className="px-2 py-1 text-xs text-ink4">No subfolders.</p> : null;

  return (
    <ul>
      {folders.map((f) => (
        <FolderNode
          key={f.id}
          loadChildren={loadChildren}
          folder={f}
          selected={selected}
          excluded={excluded}
          onChange={onChange}
          ancestorIncluded={ancestorIncluded}
          ancestorExcluded={ancestorExcluded}
          depth={depth}
        />
      ))}
    </ul>
  );
}

/** One folder row: a checkbox + an expand caret that lazy-loads its subfolders. */
function FolderNode({
  loadChildren,
  folder,
  selected,
  excluded,
  onChange,
  ancestorIncluded,
  ancestorExcluded,
  depth,
}: {
  loadChildren: LoadChildren;
  folder: PickerFolder;
  selected: string[];
  excluded: string[];
  onChange: (next: FolderSelection) => void;
  ancestorIncluded: boolean;
  ancestorExcluded: boolean;
  depth: number;
}) {
  const [open, setOpen] = useState(false);
  const isExcluded = excluded.includes(folder.id);
  // Effectively indexed: not excluded (nor under an excluded parent) and either explicitly picked or
  // covered by an included ancestor. A folder under an excluded ancestor can't be indexed, so its
  // checkbox is disabled — the user un-excludes the parent to reach it again.
  const included =
    !ancestorExcluded && !isExcluded && (ancestorIncluded || selected.includes(folder.id));
  const disabled = ancestorExcluded;

  // Self-heal a seed root that has become redundant (under an included ancestor) or stranded (under an
  // excluded ancestor). Either way it's an invisible `selected` entry; a stranded one would otherwise
  // make the walk index a subtree the user excluded. Runs when this node renders under such an ancestor
  // — which is exactly when the inconsistency becomes observable — and no-ops (returns the same object)
  // otherwise, so it settles in one pass.
  useEffect(() => {
    const next = pruneStrandedSeed(
      { selected, excluded },
      folder.id,
      ancestorIncluded,
      ancestorExcluded,
    );
    if (next.selected !== selected) onChange(next);
  }, [folder.id, selected, excluded, ancestorIncluded, ancestorExcluded, onChange]);
  return (
    <li>
      <div className="flex items-center gap-1 text-xs" style={{ paddingLeft: depth * 12 }}>
        <button
          type="button"
          aria-label={open ? "Collapse folder" : "Expand folder"}
          onClick={() => setOpen((o) => !o)}
          className="inline-flex min-h-[var(--tap-min,24px)] min-w-[var(--tap-min,24px)] shrink-0 items-center justify-center text-ink4 hover:text-ink2"
        >
          {open ? "▾" : "▸"}
        </button>
        <label className={`flex items-center gap-1.5 py-0.5 ${disabled ? "opacity-50" : ""}`}>
          <input
            type="checkbox"
            checked={included}
            disabled={disabled}
            onChange={(e) =>
              onChange(
                applyFolderToggle(
                  { selected, excluded },
                  folder.id,
                  ancestorIncluded,
                  e.target.checked,
                ),
              )
            }
          />
          <span className="truncate text-ink2">{folder.name}</span>
        </label>
      </div>
      {open && (
        <FolderChildren
          loadChildren={loadChildren}
          parentId={folder.id}
          selected={selected}
          excluded={excluded}
          onChange={onChange}
          ancestorIncluded={included}
          ancestorExcluded={ancestorExcluded || isExcluded}
          depth={depth + 1}
        />
      )}
    </li>
  );
}
