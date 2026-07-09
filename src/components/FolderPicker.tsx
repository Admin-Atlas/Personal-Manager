// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useState } from "react";

/** The minimal folder shape the picker renders — Drive and OneDrive folder rows both satisfy it. */
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
 * The shared lazy folder tree behind the Drive and OneDrive "Choose folders" scope pickers: a
 * scrollable pane of checkbox rows, each with an expand caret that lazy-loads its subfolders via
 * `loadChildren`. Selection is flat (a folder-id list) — picking a folder means "index everything
 * inside it", so children aren't auto-checked with their parent.
 */
export function FolderPicker({
  loadChildren,
  selected,
  onToggle,
  className,
}: {
  loadChildren: LoadChildren;
  selected: string[];
  onToggle: (folderId: string, checked: boolean) => void;
  /** Extra classes on the pane (the two hosts differ only in top margin). */
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
        onToggle={onToggle}
        depth={0}
      />
      <p className="px-1 pt-1 text-[10px] text-ink4">
        Picking a folder indexes everything inside it (subfolders included).
      </p>
    </div>
  );
}

/** One lazily-loaded level of the folder tree (children of `parentId`; `null` = the root). */
function FolderChildren({
  loadChildren,
  parentId,
  selected,
  onToggle,
  depth,
}: {
  loadChildren: LoadChildren;
  parentId: string | null;
  selected: string[];
  onToggle: (folderId: string, checked: boolean) => void;
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
          onToggle={onToggle}
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
  onToggle,
  depth,
}: {
  loadChildren: LoadChildren;
  folder: PickerFolder;
  selected: string[];
  onToggle: (folderId: string, checked: boolean) => void;
  depth: number;
}) {
  const [open, setOpen] = useState(false);
  const checked = selected.includes(folder.id);
  return (
    <li>
      <div className="flex items-center gap-1 text-xs" style={{ paddingLeft: depth * 12 }}>
        <button
          type="button"
          aria-label={open ? "Collapse folder" : "Expand folder"}
          onClick={() => setOpen((o) => !o)}
          className="w-4 shrink-0 text-ink4 hover:text-ink2"
        >
          {open ? "▾" : "▸"}
        </button>
        <label className="flex items-center gap-1.5 py-0.5">
          <input
            type="checkbox"
            checked={checked}
            onChange={(e) => onToggle(folder.id, e.target.checked)}
          />
          <span className="truncate text-ink2">{folder.name}</span>
        </label>
      </div>
      {open && (
        <FolderChildren
          loadChildren={loadChildren}
          parentId={folder.id}
          selected={selected}
          onToggle={onToggle}
          depth={depth + 1}
        />
      )}
    </li>
  );
}
