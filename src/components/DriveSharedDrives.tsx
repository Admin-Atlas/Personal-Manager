// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useRef, useState } from "react";
import {
  driveSharedOwners,
  getDriveScope,
  listDriveFolders,
  listDriveSharedDrives,
  setDriveScope,
} from "../lib/ipc";
import type { DriveFolder, DriveScope, SharedDrive, SharedSelection } from "../lib/types";
import { SegmentedControl } from "./ui";

/** Sentinel `driveId` the folder picker passes to walk the **personal** My Drive (matches the
 *  backend's `MY_DRIVE_ROOT`); it's also My Drive's root-folder alias, so it doubles as the top-level
 *  `parentId`. Shared-drive ids never equal `"root"`. */
const MY_DRIVE_ROOT = "root";

/**
 * Per-account **shared-drives** manager (Connectors → Drive). My Drive (personal) is indexed whole by
 * default; shared drives (Team Drives) are **opt-in and folder-scoped by default** — they're often
 * huge and org-wide, so picking folders is the safer default, with an "entire drive" escape hatch.
 * Everything stays index-only (a pointer + summary, never the bytes). Scope changes **save
 * automatically** (no Save button); the account's single **Sync now** then applies them (indexes
 * newly-in-scope files, soft-removes — kept findable, flagged source-missing — those that fell out of
 * scope), so there's one sync action for the whole account rather than a separate one per shared drive.
 */
export function SharedDrivesManager({ email, onSaved }: { email: string; onSaved: () => void }) {
  const [scope, setScope] = useState<DriveScope | null>(null);
  const [drives, setDrives] = useState<SharedDrive[] | null>(null);
  // Shared drives already indexed by another connected account → owner email. Those rows are greyed
  // out: shared drives are de-duplicated, so only their owner indexes them (this account just shares).
  const [owners, setOwners] = useState<Record<string, string>>({});
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
      const [sc, dr, ow] = await Promise.all([
        getDriveScope(email),
        listDriveSharedDrives(email),
        driveSharedOwners(email),
      ]);
      setScope(sc);
      setDrives(dr);
      setOwners(ow);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }, [email]);

  useEffect(() => {
    void load();
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

  // Persist on every change — there's no Save button. Optimistically updates local state, then writes
  // through the serialized chain. Scope only takes effect on the next "Sync now"; saving never syncs.
  const commit = (next: DriveScope) => {
    setScope(next);
    setError(null);
    setSaving(true);
    const seq = ++saveSeq.current;
    saveChain.current = saveChain.current
      .catch(() => {})
      .then(() => setDriveScope(email, next))
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

  const setMyDrive = (on: boolean) => commit({ ...scope, my_drive: on });

  const myWhole = scope.my_drive_folders == null;

  const setMyDriveWhole = (whole: boolean) =>
    commit({ ...scope, my_drive_folders: whole ? null : [] });

  const toggleMyFolder = (folderId: string, checked: boolean) => {
    const cur = scope.my_drive_folders ?? [];
    const next = checked ? [...new Set([...cur, folderId])] : cur.filter((f) => f !== folderId);
    commit({ ...scope, my_drive_folders: next });
  };

  const toggleDrive = (drive: SharedDrive, included: boolean) =>
    commit({
      ...scope,
      shared: included
        ? [...scope.shared, { drive_id: drive.id, name: drive.name, folders: [] }]
        : scope.shared.filter((s) => s.drive_id !== drive.id),
    });

  const setWhole = (driveId: string, whole: boolean) =>
    commit({
      ...scope,
      shared: scope.shared.map((s) =>
        s.drive_id === driveId ? { ...s, folders: whole ? null : [] } : s,
      ),
    });

  const toggleFolder = (driveId: string, folderId: string, checked: boolean) =>
    commit({
      ...scope,
      shared: scope.shared.map((s) => {
        if (s.drive_id !== driveId) return s;
        const cur = s.folders ?? [];
        const next = checked ? [...new Set([...cur, folderId])] : cur.filter((f) => f !== folderId);
        return { ...s, folders: next };
      }),
    });

  return (
    <div className="mt-2 space-y-3" data-help="settings-drive-shared">
      <div>
        <label className="flex items-start gap-2 text-xs">
          <input
            type="checkbox"
            checked={scope.my_drive}
            onChange={(e) => setMyDrive(e.target.checked)}
            className="mt-0.5"
          />
          <span>
            <span className="text-ink2">My Drive (personal)</span> — index your personal drive.{" "}
            {!scope.my_drive && (
              <span className="text-ink4">
                Off: existing My Drive items stay findable, but new changes won’t sync.
              </span>
            )}
          </span>
        </label>
        {scope.my_drive && (
          <div className="mt-2 pl-5">
            <SegmentedControl
              value={myWhole ? "whole" : "folders"}
              onChange={(v) => setMyDriveWhole(v === "whole")}
              options={[
                { value: "whole", label: "Entire drive" },
                { value: "folders", label: "Choose folders" },
              ]}
            />
            {!myWhole && (
              <FolderPicker
                email={email}
                driveId={MY_DRIVE_ROOT}
                selected={scope.my_drive_folders ?? []}
                onToggle={toggleMyFolder}
              />
            )}
          </div>
        )}
      </div>

      <div>
        <div className="font-mono text-[10px] uppercase tracking-wide text-ink4">Shared drives</div>
        {drives && drives.length === 0 ? (
          <p className="mt-1 text-xs text-ink4">No shared drives are available on this account.</p>
        ) : (
          <ul className="mt-2 divide-y divide-rule">
            {drives?.map((d) => {
              const sel = selectionFor(d.id);
              const whole = sel != null && sel.folders == null;
              // Already indexed by another connected account → greyed out (de-duplicated: only the
              // owner indexes a shared drive; this account would index the very same files).
              const ownedBy = owners[d.id];
              if (ownedBy) {
                return (
                  <li key={d.id} className="py-2 first:pt-0 last:pb-0 opacity-60">
                    <div className="flex items-center gap-2 text-xs">
                      <input type="checkbox" checked disabled className="cursor-not-allowed" />
                      <span className="truncate text-ink3">{d.name}</span>
                    </div>
                    <p className="mt-0.5 pl-5 text-[11px] text-ink4">
                      Already synced by <span className="text-ink3">{ownedBy}</span> — shared drives
                      are indexed once across your accounts.
                    </p>
                  </li>
                );
              }
              return (
                <li key={d.id} className="py-2 first:pt-0 last:pb-0">
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

      <p className="text-[11px] text-ink4">
        {saving ? "Saving…" : "Changes saved"} — applied next time you{" "}
        <span className="text-ink3">Sync now</span> above.
      </p>
    </div>
  );
}

/** Lazy folder tree for one drive — `driveId` is a shared drive's id (its root == the drive id) or
 *  the `MY_DRIVE_ROOT` sentinel for the personal My Drive; either way it's also the top `parentId`. */
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
    <div className="mt-2 max-h-56 overflow-auto rounded-[var(--radius)] bg-surface p-1">
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
