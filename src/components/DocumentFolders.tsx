// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The folder half of the Documents tab's Folders and Projects views (#1030): a breadcrumb back up,
// and the open folder's subfolders as tiles. The files directly in the folder are the Documents
// table underneath, the same one the All files view shows, so sorting and columns carry over.

import type { FolderNode } from "../lib/documentFolders";

const plural = (n: number) => `${n} file${n === 1 ? "" : "s"}`;

function FolderIcon() {
  return (
    <svg
      viewBox="0 0 24 24"
      aria-hidden
      className="h-5 w-5 shrink-0 text-ink3"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.75}
      strokeLinejoin="round"
    >
      <path d="M3 7.5a2 2 0 0 1 2-2h4l2 2.5h8a2 2 0 0 1 2 2V17a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
    </svg>
  );
}

/** Where you are: the top, then each folder down to the open one. Every crumb but the last opens
 *  its folder. */
export function FolderCrumbs({
  trail,
  rootLabel,
  onOpen,
}: {
  /** Root first, the open folder last (`openFolder`'s trail). */
  trail: FolderNode[];
  rootLabel: string;
  /** Open the folder `depth` steps down from the top (0 = the top). */
  onOpen: (depth: number) => void;
}) {
  return (
    <nav aria-label="Folder" className="mb-3 flex min-w-0 flex-wrap items-center gap-1 text-sm">
      {trail.map((node, depth) => {
        const last = depth === trail.length - 1;
        const label = depth === 0 ? rootLabel : node.name;
        return (
          <span key={`${depth}-${node.key}`} className="flex min-w-0 items-center gap-1">
            {depth > 0 && (
              <span className="text-ink4" aria-hidden>
                ›
              </span>
            )}
            {last ? (
              <span aria-current="page" className="truncate font-medium text-ink">
                {label}
              </span>
            ) : (
              <button
                type="button"
                onClick={() => onOpen(depth)}
                className="truncate rounded-[var(--radius-sm)] px-1 text-ink3 hover:bg-surface hover:text-ink"
              >
                {label}
              </button>
            )}
          </span>
        );
      })}
    </nav>
  );
}

/** The open folder's subfolders, as tiles you click into. */
export function FolderGrid({
  folders,
  onOpen,
}: {
  folders: FolderNode[];
  onOpen: (key: string) => void;
}) {
  if (folders.length === 0) return null;
  return (
    <ul className="mb-4 grid grid-cols-[repeat(auto-fill,minmax(13rem,1fr))] gap-2">
      {folders.map((f) => (
        <li key={f.key}>
          <button
            type="button"
            onClick={() => onOpen(f.key)}
            title={f.detail ? `${f.name} · ${f.detail}` : f.name}
            className="flex w-full items-center gap-2.5 rounded-[var(--radius-sm)] border border-border px-3 py-2 text-left hover:border-border2 hover:bg-surface"
          >
            <FolderIcon />
            <span className="min-w-0 flex-1">
              <span className="block truncate text-sm text-ink">{f.name}</span>
              <span className="block truncate text-xs text-ink4">
                {f.detail ? `${f.detail} · ${plural(f.total)}` : plural(f.total)}
              </span>
            </span>
          </button>
        </li>
      ))}
    </ul>
  );
}
