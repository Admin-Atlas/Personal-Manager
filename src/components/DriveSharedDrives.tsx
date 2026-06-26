// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useState } from "react";
import { getDriveScope, listDriveFolders, listDriveSharedDrives, setDriveScope } from "../lib/ipc";
import type { DriveFolder, DriveScope, SharedDrive, SharedSelection } from "../lib/types";
import { Button, SegmentedControl } from "./ui";

/**
 * Per-account **shared-drives** manager (Connectors → Drive). My Drive (personal) is indexed whole by
 * default; shared drives (Team Drives) are **opt-in and folder-scoped by default** — they're often
 * huge and org-wide, so picking folders is the safer default, with an "entire drive" escape hatch.
 * Everything stays index-only (a pointer + summary, never the bytes). **Save** only persists the
 * scope; the account's single **Sync now** then applies it (indexes newly-in-scope files, soft-removes
 * — kept findable, flagged source-missing — those that fell out of scope), so there's one sync action
 * for the whole account rather than a separate one per shared drive.
 */
export function SharedDrivesManager({
  email,
  busy,
  onSaved,
}: {
  email: string;
  busy: boolean;
  onSaved: () => void;
}) {
  const [scope, setScope] = useState<DriveScope | null>(null);
  const [drives, setDrives] = useState<SharedDrive[] | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const [sc, dr] = await Promise.all([getDriveScope(email), listDriveSharedDrives(email)]);
      setScope(sc);
      setDrives(dr);
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
        {error ?? "Loading shared drives…"}
        {error && (
          <button type="button" onClick={load} className="ml-2 underline">
            Retry
          </button>
        )}
      </p>
    );
  }

  const selectionFor = (driveId: string): SharedSelection | undefined =>
    scope.shared.find((s) => s.drive_id === driveId);

  const setMyDrive = (on: boolean) => setScope({ ...scope, my_drive: on });

  const toggleDrive = (drive: SharedDrive, included: boolean) =>
    setScope({
      ...scope,
      shared: included
        ? [...scope.shared, { drive_id: drive.id, name: drive.name, folders: [] }]
        : scope.shared.filter((s) => s.drive_id !== drive.id),
    });

  const setWhole = (driveId: string, whole: boolean) =>
    setScope({
      ...scope,
      shared: scope.shared.map((s) =>
        s.drive_id === driveId ? { ...s, folders: whole ? null : [] } : s,
      ),
    });

  const toggleFolder = (driveId: string, folderId: string, checked: boolean) =>
    setScope({
      ...scope,
      shared: scope.shared.map((s) => {
        if (s.drive_id !== driveId) return s;
        const cur = s.folders ?? [];
        const next = checked ? [...new Set([...cur, folderId])] : cur.filter((f) => f !== folderId);
        return { ...s, folders: next };
      }),
    });

  const save = async () => {
    setSaving(true);
    setError(null);
    try {
      await setDriveScope(email, scope);
      onSaved();
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  return (
    <div className="mt-2 space-y-3" data-help="settings-drive-shared">
      <label className="flex items-start gap-2 text-xs">
        <input
          type="checkbox"
          checked={scope.my_drive}
          onChange={(e) => setMyDrive(e.target.checked)}
          className="mt-0.5"
        />
        <span>
          <span className="text-ink2">My Drive (personal)</span> — index your whole personal drive.{" "}
          {!scope.my_drive && (
            <span className="text-ink4">
              Off: existing My Drive items stay findable, but new changes won’t sync.
            </span>
          )}
        </span>
      </label>

      <div>
        <div className="font-mono text-[10px] uppercase tracking-wide text-ink4">Shared drives</div>
        {drives && drives.length === 0 ? (
          <p className="mt-1 text-xs text-ink4">No shared drives are available on this account.</p>
        ) : (
          <ul className="mt-2 space-y-2">
            {drives?.map((d) => {
              const sel = selectionFor(d.id);
              const whole = sel != null && sel.folders == null;
              return (
                <li key={d.id} className="rounded-[var(--radius)] border border-border p-2">
                  <label className="flex items-center gap-2 text-xs">
                    <input
                      type="checkbox"
                      checked={sel != null}
                      onChange={(e) => toggleDrive(d, e.target.checked)}
                    />
                    <span className="truncate text-ink2">{d.name}</span>
                  </label>
                  {sel != null && (
                    <div className="mt-2 pl-5">
                      <SegmentedControl
                        value={whole ? "whole" : "folders"}
                        onChange={(v) => setWhole(d.id, v === "whole")}
                        options={[
                          { value: "folders", label: "Choose folders" },
                          { value: "whole", label: "Entire drive" },
                        ]}
                      />
                      {!whole && (
                        <FolderPicker
                          email={email}
                          driveId={d.id}
                          selected={sel.folders ?? []}
                          onToggle={(fid, checked) => toggleFolder(d.id, fid, checked)}
                        />
                      )}
                    </div>
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </div>

      {error && <p className="text-xs text-st-due">{error}</p>}

      <div className="flex items-center gap-2">
        <Button onClick={save} disabled={busy || saving} className="px-2 py-1 text-xs">
          {saving ? "Saving…" : "Save"}
        </Button>
        <span className="text-[11px] text-ink4">
          Saved here, then indexed when you <span className="text-ink3">Sync now</span> above.
        </span>
      </div>
    </div>
  );
}

/** Lazy folder tree for one shared drive — the drive root's id equals the drive id. */
function FolderPicker({
  email,
  driveId,
  selected,
  onToggle,
}: {
  email: string;
  driveId: string;
  selected: string[];
  onToggle: (folderId: string, checked: boolean) => void;
}) {
  return (
    <div className="mt-2 max-h-56 overflow-auto rounded-[var(--radius)] border border-border p-1">
      <FolderChildren
        email={email}
        driveId={driveId}
        parentId={driveId}
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

/** One lazily-loaded level of the folder tree (children of `parentId`). */
function FolderChildren({
  email,
  driveId,
  parentId,
  selected,
  onToggle,
  depth,
}: {
  email: string;
  driveId: string;
  parentId: string;
  selected: string[];
  onToggle: (folderId: string, checked: boolean) => void;
  depth: number;
}) {
  const [folders, setFolders] = useState<DriveFolder[] | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let active = true;
    listDriveFolders(email, driveId, parentId)
      .then((f) => active && setFolders(f))
      .catch((e) => active && setError(String(e)));
    return () => {
      active = false;
    };
  }, [email, driveId, parentId]);

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
          driveId={driveId}
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
  driveId,
  folder,
  selected,
  onToggle,
  depth,
}: {
  email: string;
  driveId: string;
  folder: DriveFolder;
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
          driveId={driveId}
          parentId={folder.id}
          selected={selected}
          onToggle={onToggle}
          depth={depth + 1}
        />
      )}
    </li>
  );
}
