// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useRef, useState } from "react";
import { getOneDriveScope, listOneDriveFolders, setOneDriveScope } from "../lib/ipc";
import type { OneDriveFolder, OneDriveScope } from "../lib/types";
import { SegmentedControl } from "./ui";

/**
 * Per-account OneDrive **scope** manager (Connectors → Drive). The personal OneDrive is indexed whole
 * by default (the efficient delta cursor); switch to "Choose folders" to index only the folders you
 * pick (everything inside, recursively — re-enumerated each sync). Everything stays index-only (a
 * pointer + summary, never the bytes). Scope changes **save automatically** (no Save button); the
 * account's **Sync now** then applies them (indexes newly-in-scope files, soft-removes — kept
 * findable, flagged source-missing — those that fell out of scope).
 *
 * The Drive sibling ({@link "./DriveSharedDrives"}) also lists shared drives; OneDrive has just the
 * one personal drive, so this is only the My-Drive-style whole/folders choice.
 */
export function OneDriveFolders({ email, onSaved }: { email: string; onSaved: () => void }) {
  const [scope, setScope] = useState<OneDriveScope | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  // Serialize auto-saves (see `commit`): writes go out in click order, and only the latest one drives
  // the "Saving…/Saved" flag so a burst of folder toggles can't land out of order or get stuck.
  const saveSeq = useRef(0);
  const saveChain = useRef<Promise<void>>(Promise.resolve());

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setScope(await getOneDriveScope(email));
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [email]);

  useEffect(() => {
    load();
  }, [load]);

  if (loading || scope == null) {
    return (
      <p className="mt-2 text-xs text-ink4">
        {error ?? "Loading scope…"}
        {error && (
          <button type="button" onClick={load} className="ml-2 underline">
            Retry
          </button>
        )}
      </p>
    );
  }

  const whole = scope.folders == null;

  // Persist on every change — there's no Save button. Optimistically updates local state, then writes
  // through the serialized chain. Scope only takes effect on the next "Sync now"; saving never syncs.
  const commit = (next: OneDriveScope) => {
    setScope(next);
    setError(null);
    setSaving(true);
    const seq = ++saveSeq.current;
    saveChain.current = saveChain.current
      .catch(() => {})
      .then(() => setOneDriveScope(email, next))
      .then(() => {
        if (seq !== saveSeq.current) return; // a newer save is in flight; let it own the UI
        setSaving(false);
        onSaved();
      })
      .catch((e) => {
        if (seq !== saveSeq.current) return;
        setSaving(false);
        setError(String(e));
      });
  };

  const setWhole = (w: boolean) => commit({ folders: w ? null : [] });

  const toggleFolder = (folderId: string, checked: boolean) => {
    const cur = scope.folders ?? [];
    const next = checked ? [...new Set([...cur, folderId])] : cur.filter((f) => f !== folderId);
    commit({ folders: next });
  };

  return (
    <div className="mt-2 space-y-3" data-help="settings-onedrive-scope">
      <SegmentedControl
        value={whole ? "whole" : "folders"}
        onChange={(v) => setWhole(v === "whole")}
        options={[
          { value: "whole", label: "Entire OneDrive" },
          { value: "folders", label: "Choose folders" },
        ]}
      />
      {!whole && (
        <FolderPicker email={email} selected={scope.folders ?? []} onToggle={toggleFolder} />
      )}

      {error && <p className="text-xs text-st-due">{error}</p>}

      <p className="text-[11px] text-ink4">
        {saving ? "Saving…" : "Changes saved"} — applied next time you{" "}
        <span className="text-ink3">Sync now</span> above.
      </p>
    </div>
  );
}

/** Lazy folder tree rooted at the drive root (`parentId = null`). */
function FolderPicker({
  email,
  selected,
  onToggle,
}: {
  email: string;
  selected: string[];
  onToggle: (folderId: string, checked: boolean) => void;
}) {
  return (
    <div className="mt-1 max-h-56 overflow-auto rounded-[var(--radius)] bg-surface p-1">
      <FolderChildren
        email={email}
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

/** One lazily-loaded level of the folder tree (children of `parentId`; `null` = the drive root). */
function FolderChildren({
  email,
  parentId,
  selected,
  onToggle,
  depth,
}: {
  email: string;
  parentId: string | null;
  selected: string[];
  onToggle: (folderId: string, checked: boolean) => void;
  depth: number;
}) {
  const [folders, setFolders] = useState<OneDriveFolder[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    listOneDriveFolders(email, parentId)
      .then((f) => active && setFolders(f))
      .catch((e) => active && setError(String(e)));
    return () => {
      active = false;
    };
  }, [email, parentId]);

  if (error) return <p className="px-2 py-1 text-xs text-st-due">{error}</p>;
  if (folders == null) return <p className="px-2 py-1 text-xs text-ink4">Loading…</p>;
  if (folders.length === 0)
    return depth === 0 ? <p className="px-2 py-1 text-xs text-ink4">No subfolders.</p> : null;

  return (
    <ul>
      {folders.map((f) => (
        <FolderNode
          key={f.id}
          email={email}
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
  email,
  folder,
  selected,
  onToggle,
  depth,
}: {
  email: string;
  folder: OneDriveFolder;
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
          email={email}
          parentId={folder.id}
          selected={selected}
          onToggle={onToggle}
          depth={depth + 1}
        />
      )}
    </li>
  );
}
