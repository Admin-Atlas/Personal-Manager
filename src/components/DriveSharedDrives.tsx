// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  driveSharedOwners,
  driveSwmRootOwners,
  getDriveScope,
  listDriveFolders,
  listDriveSharedDrives,
  listDriveSharedWithMeRoots,
  setDriveScope,
} from "../lib/ipc";
import type { DriveScope, SharedDrive, SharedSelection, SwmRoot } from "../lib/types";
import { FolderPicker } from "./FolderPicker";
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

  // The scope is a plain DB read and is what the My Drive checkbox and folder chooser need; the other
  // two are live Drive calls. They used to share one Promise.all, so a single network failure left
  // `scope` null and replaced the ENTIRE editor — including the controls that need no network at all
  // — with one error line. Load the local half first and let the remote half fail on its own.
  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setScope(await getDriveScope(email));
    } catch (e) {
      setError(String(e));
      setLoading(false);
      return;
    }
    try {
      const [dr, ow] = await Promise.all([listDriveSharedDrives(email), driveSharedOwners(email)]);
      setDrives(dr);
      setOwners(ow);
    } catch (e) {
      // Non-fatal: the shared-drive list carries its own inline error + Retry below.
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

  // A mode switch resets any excludes (they only mean anything alongside chosen folders).
  const setMyDriveWhole = (whole: boolean) =>
    commit({ ...scope, my_drive_folders: whole ? null : [], my_drive_exclude: [] });

  const setMyFolders = (selected: string[], excluded: string[]) =>
    commit({ ...scope, my_drive_folders: selected, my_drive_exclude: excluded });

  const toggleDrive = (drive: SharedDrive, included: boolean) =>
    commit({
      ...scope,
      shared: included
        ? [...scope.shared, { drive_id: drive.id, name: drive.name, folders: [], exclude: [] }]
        : scope.shared.filter((s) => s.drive_id !== drive.id),
    });

  const setWhole = (driveId: string, whole: boolean) =>
    commit({
      ...scope,
      shared: scope.shared.map((s) =>
        s.drive_id === driveId ? { ...s, folders: whole ? null : [], exclude: [] } : s,
      ),
    });

  const setDriveFolders = (driveId: string, selected: string[], excluded: string[]) =>
    commit({
      ...scope,
      shared: scope.shared.map((s) =>
        s.drive_id === driveId ? { ...s, folders: selected, exclude: excluded } : s,
      ),
    });

  // Root-file opt-ins (folder-scoped only) — see [`RootFilesToggle`].
  const setMyRootFiles = (on: boolean) => commit({ ...scope, my_drive_include_root_files: on });
  const setDriveRootFiles = (driveId: string, on: boolean) =>
    commit({
      ...scope,
      shared: scope.shared.map((s) =>
        s.drive_id === driveId ? { ...s, include_root_files: on } : s,
      ),
    });

  // Shared with me: a master toggle plus Everything (`roots == null`) vs Choose (a picked-id list).
  const swmWhole = scope.shared_with_me_roots == null;
  const setSharedWithMe = (on: boolean) =>
    commit({
      ...scope,
      shared_with_me: on,
      // A freshly-enabled corpus starts in "Choose" (empty) so it doesn't flood the review queue.
      shared_with_me_roots: on ? (scope.shared_with_me_roots ?? []) : scope.shared_with_me_roots,
    });
  const setSwmWhole = (whole: boolean) =>
    commit({ ...scope, shared_with_me_roots: whole ? null : [] });
  const setSwmRoots = (ids: string[]) => commit({ ...scope, shared_with_me_roots: ids });

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
              ariaLabel="My Drive scope"
              value={myWhole ? "whole" : "folders"}
              onChange={(v) => setMyDriveWhole(v === "whole")}
              options={[
                { value: "whole", label: "Entire drive" },
                { value: "folders", label: "Choose folders" },
              ]}
            />
            {!myWhole && (
              <>
                <DriveFolderPicker
                  email={email}
                  driveId={MY_DRIVE_ROOT}
                  selected={scope.my_drive_folders ?? []}
                  excluded={scope.my_drive_exclude ?? []}
                  onChange={setMyFolders}
                />
                <RootFilesToggle
                  checked={scope.my_drive_include_root_files ?? false}
                  onChange={setMyRootFiles}
                />
              </>
            )}
          </div>
        )}
      </div>

      <div>
        <div className="font-mono text-[0.625rem] uppercase tracking-wide text-ink4">
          Shared drives
        </div>
        {drives == null && error ? (
          // The live half failed while the scope read succeeded — say so here, next to the list that
          // is missing, instead of blanking the whole editor.
          <p className="mt-1 break-words text-xs text-st-due">
            Couldn&rsquo;t list shared drives: {error}
            <button type="button" onClick={() => void load()} className="ml-2 underline">
              Retry
            </button>
          </p>
        ) : drives && drives.length === 0 ? (
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
                    <p className="mt-0.5 pl-5 text-[0.6875rem] text-ink4">
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
                        ariaLabel={`${d.name} scope`}
                        value={whole ? "whole" : "folders"}
                        onChange={(v) => setWhole(d.id, v === "whole")}
                        options={[
                          { value: "folders", label: "Choose folders" },
                          { value: "whole", label: "Entire drive" },
                        ]}
                      />
                      {!whole && (
                        <>
                          <DriveFolderPicker
                            email={email}
                            driveId={d.id}
                            selected={sel.folders ?? []}
                            excluded={sel.exclude ?? []}
                            onChange={(selected, excluded) =>
                              setDriveFolders(d.id, selected, excluded)
                            }
                          />
                          <RootFilesToggle
                            checked={sel.include_root_files ?? false}
                            onChange={(on) => setDriveRootFiles(d.id, on)}
                          />
                        </>
                      )}
                    </div>
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </div>

      <div>
        <div className="font-mono text-[0.625rem] uppercase tracking-wide text-ink4">
          Shared with me
        </div>
        <label className="mt-2 flex items-start gap-2 text-xs">
          <input
            type="checkbox"
            checked={scope.shared_with_me ?? false}
            onChange={(e) => setSharedWithMe(e.target.checked)}
            className="mt-0.5"
          />
          <span>
            <span className="text-ink2">Files &amp; folders shared with you</span> — index items
            others shared directly with this account (separate from My Drive and shared drives).{" "}
            {!scope.shared_with_me && (
              <span className="text-ink4">Off by default — this collection can be large.</span>
            )}
          </span>
        </label>
        {scope.shared_with_me && (
          <div className="mt-2 pl-5">
            <SegmentedControl
              ariaLabel="Shared with me scope"
              value={swmWhole ? "whole" : "choose"}
              onChange={(v) => setSwmWhole(v === "whole")}
              options={[
                { value: "choose", label: "Choose items" },
                { value: "whole", label: "Everything" },
              ]}
            />
            {!swmWhole && (
              <SharedWithMeRoots
                email={email}
                selected={scope.shared_with_me_roots ?? []}
                onChange={setSwmRoots}
              />
            )}
          </div>
        )}
      </div>

      {error && <p className="text-xs text-st-due">{error}</p>}

      <p className="text-[0.6875rem] text-ink4">
        {saving ? "Saving…" : "Changes saved"} — applied next time you{" "}
        <span className="text-ink3">Sync now</span> above.
      </p>
    </div>
  );
}

/** Lazy folder tree for one drive (the shared {@link FolderPicker}) — `driveId` is a shared drive's
 *  id (its root == the drive id) or the `MY_DRIVE_ROOT` sentinel for the personal My Drive; either
 *  way it doubles as the top-level parent, so the picker's `null` root maps to it. */
function DriveFolderPicker({
  email,
  driveId,
  selected,
  excluded,
  onChange,
}: {
  email: string;
  driveId: string;
  selected: string[];
  excluded: string[];
  onChange: (selected: string[], excluded: string[]) => void;
}) {
  // Stable per (email, driveId) so the picker's lazy-load effects don't refire on every render.
  const loadChildren = useCallback(
    (parentId: string | null) => listDriveFolders(email, driveId, parentId ?? driveId),
    [email, driveId],
  );
  return (
    <FolderPicker
      className="mt-2"
      loadChildren={loadChildren}
      selected={selected}
      excluded={excluded}
      onChange={(next) => onChange(next.selected, next.excluded)}
    />
  );
}

/** The "Files in the drive root" opt-in shown under a folder-scoped selection — loose files at a
 *  drive's top level aren't inside any folder, so the folder picker alone can't reach them. */
function RootFilesToggle({
  checked,
  onChange,
}: {
  checked: boolean;
  onChange: (on: boolean) => void;
}) {
  return (
    <label className="mt-2 flex items-start gap-2 text-[0.6875rem]">
      <input
        type="checkbox"
        checked={checked}
        onChange={(e) => onChange(e.target.checked)}
        className="mt-0.5"
      />
      <span className="text-ink3">
        Also index files in the drive’s root (loose files not inside any folder).
      </span>
    </label>
  );
}

/** How many rows this picker shows before it becomes its own scroll area. Roughly a screenful of the
 *  connector card — enough that a handful of shared items needs no scrolling at all, while an account
 *  with hundreds can't push the rest of the settings (and the Save) off the page. */
const VISIBLE_ROOTS = 8;

/** The flat picker of "Shared with me" roots — files AND folders (a folder pulls in its whole
 *  subtree; a trailing “/” marks a folder). Roots already indexed by another connected account are
 *  greyed out (de-duplicated like a shared drive). Loads its own roots + owners on mount, so the list
 *  is fetched only when the "Choose items" view is actually shown. */
function SharedWithMeRoots({
  email,
  selected,
  onChange,
}: {
  email: string;
  selected: string[];
  onChange: (ids: string[]) => void;
}) {
  const [roots, setRoots] = useState<SwmRoot[] | null>(null);
  const [owners, setOwners] = useState<Record<string, string>>({});
  const [error, setError] = useState<string | null>(null);
  /** Drive returns no order at all, so the picker picks one. Session-only — a picker's ordering is
   *  not worth a stored preference. */
  const [rootSort, setRootSort] = useState<"recent" | "name">("recent");

  const load = useCallback(async () => {
    setError(null);
    try {
      const [rs, ow] = await Promise.all([
        listDriveSharedWithMeRoots(email),
        driveSwmRootOwners(email),
      ]);
      setRoots(rs);
      setOwners(ow);
    } catch (e) {
      setError(String(e));
    }
  }, [email]);

  useEffect(() => {
    void load();
  }, [load]);

  // Drive returns these in no particular order (the listing sets no `orderBy`), which is fine for a
  // handful and useless for fifty. "Recent" is when it was shared WITH YOU, not when the file was last
  // modified — for finding the thing someone just sent you, that is the order you actually want.
  // ISO-8601 sorts lexically, so no parsing; an item with no timestamp sinks rather than jumping.
  //
  // Above the early returns below: hooks must run in the same order on every render.
  const sortedRoots = useMemo(() => {
    const copy = [...(roots ?? [])];
    if (rootSort === "name") copy.sort((a, b) => a.name.localeCompare(b.name));
    else
      copy.sort((a, b) => (b.shared_with_me_time ?? "").localeCompare(a.shared_with_me_time ?? ""));
    return copy;
  }, [roots, rootSort]);

  if (error) {
    return (
      <p className="mt-2 text-xs text-st-due">
        {error}
        <button type="button" onClick={load} className="ml-2 underline">
          Retry
        </button>
      </p>
    );
  }
  if (roots == null) {
    return <p className="mt-2 text-xs text-ink4">Loading shared items…</p>;
  }
  if (roots.length === 0) {
    return <p className="mt-2 text-xs text-ink4">Nothing has been shared with this account.</p>;
  }

  const sel = new Set(selected);
  const toggle = (id: string, on: boolean) => {
    const next = new Set(sel);
    if (on) next.add(id);
    else next.delete(id);
    onChange([...next]);
  };

  const chosen = roots.filter((r) => sel.has(r.id) || owners[r.id]).length;

  return (
    <>
      <p className="mt-2 flex items-center justify-between gap-2 text-[0.6875rem] text-ink4">
        <span>
          {roots.length} shared item{roots.length === 1 ? "" : "s"}
          {chosen > 0 ? ` · ${chosen} selected` : ""}
        </span>
        <span className="flex items-center gap-2">
          {roots.length > VISIBLE_ROOTS && <span>scroll for more</span>}
          <SegmentedControl
            ariaLabel="Sort shared items"
            value={rootSort}
            onChange={setRootSort}
            options={[
              { value: "recent", label: "Recent", title: "Most recently shared with you first" },
              { value: "name", label: "Name", title: "A to Z" },
            ]}
          />
        </span>
      </p>
      {/* Capped and scrolled past a handful of rows: an account with a lot of shared items otherwise
          pushed the rest of the connector settings (and its own Save) off the page. The cap is on the
          LIST, not the page, so the surrounding controls stay put however much is shared. */}
      <ul
        className={`mt-1 divide-y divide-rule ${
          roots.length > VISIBLE_ROOTS
            ? "max-h-64 overflow-y-auto rounded-[var(--radius-sm)] border border-border px-2"
            : ""
        }`}
      >
        {sortedRoots.map((r) => {
          const ownedBy = owners[r.id];
          return (
            <li key={r.id} className="py-1.5 first:pt-0 last:pb-0">
              <label className={`flex items-center gap-2 text-xs ${ownedBy ? "opacity-60" : ""}`}>
                <input
                  type="checkbox"
                  checked={ownedBy ? true : sel.has(r.id)}
                  disabled={!!ownedBy}
                  onChange={(e) => toggle(r.id, e.target.checked)}
                  className={`shrink-0 ${ownedBy ? "cursor-not-allowed" : ""}`}
                />
                <span className="truncate text-ink2">
                  {r.name}
                  {r.is_folder ? "/" : ""}
                </span>
              </label>
              {/* Who sent it. Rendered only when Drive reported someone: it names the sharer on a
                  directly-shared root, and a name-only row degrades silently rather than showing an
                  empty "Shared by". Distinct from "Already synced by", which is one of YOUR accounts. */}
              {r.shared_by && (
                <p className="mt-0.5 pl-5 text-[0.6875rem] text-ink4">
                  Shared by <span className="text-ink3">{r.shared_by}</span>
                </p>
              )}
              {ownedBy && (
                <p className="mt-0.5 pl-5 text-[0.6875rem] text-ink4">
                  Already synced by <span className="text-ink3">{ownedBy}</span>.
                </p>
              )}
            </li>
          );
        })}
      </ul>
    </>
  );
}
