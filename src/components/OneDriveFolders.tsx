// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useRef, useState } from "react";
import { getOneDriveScope, listOneDriveFolders, setOneDriveScope } from "../lib/ipc";
import type { OneDriveScope } from "../lib/types";
import { FolderPicker } from "./FolderPicker";
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

  // Loader for the shared FolderPicker: OneDrive has one drive, so `null` (the picker's root) passes
  // straight through. Stable per email so the picker's lazy-load effects don't refire every render.
  const loadChildren = useCallback(
    (parentId: string | null) => listOneDriveFolders(email, parentId),
    [email],
  );

  useEffect(() => {
    void load();
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

  // A mode switch resets any excludes (they only mean anything alongside chosen folders).
  const setWhole = (w: boolean) => commit({ folders: w ? null : [], exclude: [] });

  return (
    <div className="mt-2 space-y-3" data-help="settings-onedrive-scope">
      <SegmentedControl
        ariaLabel="OneDrive scope"
        value={whole ? "whole" : "folders"}
        onChange={(v) => setWhole(v === "whole")}
        options={[
          { value: "whole", label: "Entire OneDrive" },
          { value: "folders", label: "Choose folders" },
        ]}
      />
      {!whole && (
        <FolderPicker
          className="mt-1"
          loadChildren={loadChildren}
          selected={scope.folders ?? []}
          excluded={scope.exclude ?? []}
          onChange={({ selected, excluded }) => commit({ folders: selected, exclude: excluded })}
        />
      )}

      {error && <p className="text-xs text-st-due">{error}</p>}

      <p className="text-[0.6875rem] text-ink4">
        {saving ? "Saving…" : "Changes saved"} — applied next time you{" "}
        <span className="text-ink3">Sync now</span> above.
      </p>
    </div>
  );
}
