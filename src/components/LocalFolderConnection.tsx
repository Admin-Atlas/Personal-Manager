// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  addLocalFolder,
  listLocalFolders,
  listLocalSubfolders,
  localFolderSyncStatus,
  onLocalChanged,
  onLocalSync,
  removeLocalFolder,
  setLocalExcludes,
  stopLocalFolderSync,
  syncLocalFolder,
} from "../lib/ipc";
import type { LocalFolder } from "../lib/types";
import { useDetachedSync } from "../lib/useDetachedSync";
import { formatWhen } from "../lib/format";
import { Button, ConfirmDialog } from "./ui";
import { FolderPicker } from "./FolderPicker";
import { SyncProgress } from "./SyncProgress";
import { SyncReport } from "./SyncReport";
import { ConnectorItemRow } from "./ConnectorItemRow";

/**
 * **Local folders** (index-only) — the filesystem connector (board card 6), under the Connectors tab.
 * Unlike the cloud connectors it has no provider and no sign-in: you point PM at a folder on this
 * machine and it indexes the ingestible files inside it. Every file is **index-only** — PM stores a
 * searchable pointer + a short summary, never the bytes; the file stays on disk and is read on demand.
 *
 * After a folder is added and synced once, a live filesystem **watcher** keeps it current in the
 * background (edits re-embed, renames/moves keep the same item, deletes go soft, unmount → unreachable),
 * so "Sync now" is mostly a manual catch-up. The watcher emits `local://changed` when it applies a batch
 * ({@link useDetachedSync}'s `watch`); we refetch the list on that so counts and state badges stay live.
 *
 * The sync runs **detached** in the backend and single-flights, exactly like the Drive/OneDrive
 * connectors — this view shares their {@link useDetachedSync} state machine and their progress/report/stop
 * UI, and restores an in-flight sync's bar from the backend snapshot on (re)mount.
 */
export function LocalFolderConnection() {
  const [folders, setFolders] = useState<LocalFolder[]>([]);
  const [confirmRemove, setConfirmRemove] = useState<LocalFolder | null>(null);
  // Which folder's subfolder picker is expanded (one at a time), keyed by folder key.
  const [pickerKey, setPickerKey] = useState<string | null>(null);

  const refreshRef = useRef<() => void>(() => {});
  const ds = useDetachedSync<Awaited<ReturnType<typeof localFolderSyncStatus>>>({
    subscribe: onLocalSync,
    fetchStatus: localFolderSyncStatus,
    targetOf: (s) => s.folder,
    start: syncLocalFolder,
    stop: stopLocalFolderSync,
    onSettled: () => refreshRef.current(),
    watch: onLocalChanged,
  });
  const { busy, error, setError, syncing, target: syncKey, queued, report, progress } = ds;

  const refresh = useCallback(async () => {
    try {
      setFolders(await listLocalFolders());
    } catch (e) {
      setError(String(e));
    }
  }, [setError]);
  useEffect(() => {
    refreshRef.current = () => void refresh();
  }, [refresh]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // Native folder picker → register → index it right away. There's no scope to choose (unlike Drive's
  // My-Drive-vs-folders), so adding a folder means "index this" — we kick off its first sync at once.
  const add = () =>
    ds.run("add", async () => {
      const selected = await open({ directory: true });
      if (!selected) return;
      const key = await addLocalFolder(selected as string);
      await refresh();
      ds.sync(key);
    });

  const remove = (key: string) =>
    ds.run("remove", async () => {
      await removeLocalFolder(key);
      await refresh();
    });

  const anyBusy = busy != null || syncing;

  return (
    <div data-help="settings-local-folders">
      <span className="text-sm font-medium text-ink">Local folders</span>
      <p className="mt-1 text-xs text-ink4">
        Index a folder on this computer. Everything is <em>index-only</em> — a searchable pointer
        and a short summary; the file itself stays on disk and is read on demand. Once indexed, PM{" "}
        <strong>watches the folder</strong> and keeps it current as files change — no re-sync
        needed.
      </p>

      {folders.length > 0 && (
        <ul className="mt-3 divide-y divide-rule rounded-[var(--radius)] border border-border">
          {folders.map((f) => (
            <li key={f.key} className="px-3 py-2">
              <ConnectorItemRow
                title={f.label}
                // "unreachable" is the registry state (a failed sync / removed root); `present:false` is
                // a live check that the path isn't a readable directory right now — surface either.
                reachable={f.state === "ok" && f.present}
                badgeLabel={f.present ? "unreachable" : "not found"}
                detail={
                  <p className="mt-0.5 truncate text-xs text-ink4" title={f.path}>
                    {f.path}
                  </p>
                }
                meta={
                  <>
                    {f.indexed} indexed
                    {f.last_synced_at
                      ? ` · synced ${formatWhen(f.last_synced_at)}`
                      : " · not synced yet"}
                  </>
                }
                syncingThis={syncKey === f.key}
                queued={syncing && queued.has(f.key)}
                // Sync stays clickable for folders *not* currently syncing, so one can be queued
                // mid-index; only the syncing row and in-flight add/remove block it.
                syncDisabled={syncKey === f.key || busy != null}
                onSync={() => ds.sync(f.key)}
                actionLabel="Remove"
                actionDisabled={anyBusy}
                onAction={() => setConfirmRemove(f)}
              />
              {/* Subfolder excludes — only when the folder is readable right now (the picker walks it). */}
              {f.present && f.state === "ok" && (
                <div className="mt-1 pl-8">
                  <button
                    type="button"
                    className="text-[0.6875rem] text-ink4 underline hover:text-ink2"
                    onClick={() => setPickerKey((k) => (k === f.key ? null : f.key))}
                  >
                    {pickerKey === f.key ? "Hide subfolders" : "Choose subfolders"}
                    {f.exclude.length > 0 ? ` (${f.exclude.length} excluded)` : ""}
                  </button>
                  {pickerKey === f.key && (
                    <LocalFolderExcludes folder={f} onSaved={() => void refresh()} />
                  )}
                </div>
              )}
            </li>
          ))}
        </ul>
      )}

      {folders.length === 0 && !syncing && (
        <p className="mt-3 text-xs text-ink4">
          No folders tracked yet. Add one below — its documents become searchable and appear in
          Documents, and PM keeps the folder up to date as you work in it.
        </p>
      )}

      {syncing && progress && (
        <SyncProgress
          startedAt={ds.startedAt}
          processed={progress.processed}
          total={progress.total}
          label={syncKey ? "Indexing folder" : "Indexing your folders"}
          sizeQuestion="Changed your mind?"
          stopping={ds.stopping}
          confirmStop={ds.confirmStop}
          setConfirmStop={ds.setConfirmStop}
          onStop={ds.requestStop}
        />
      )}

      <div className="mt-3">
        {/* Gated on `busy` only, not `anyBusy` — you can add another folder while a sync runs. */}
        <Button
          variant={folders.length === 0 ? "primary" : "secondary"}
          onClick={add}
          disabled={busy != null}
          className="disabled:opacity-40"
        >
          {busy === "add"
            ? "Adding…"
            : folders.length === 0
              ? "Add a folder"
              : "Add another folder"}
        </Button>
      </div>

      {!syncing && report && (
        <SyncReport report={report} helpId="settings-local-report" onDismiss={ds.dismissReport} />
      )}

      {error && (
        <p
          className="mt-2 rounded-[var(--radius)] px-3 py-2 text-xs text-st-due"
          style={{ background: "color-mix(in oklab, var(--st-due) 15%, transparent)" }}
        >
          {error}
        </p>
      )}

      <ConfirmDialog
        open={confirmRemove != null}
        title="Stop tracking this folder?"
        danger
        confirmLabel="Stop tracking"
        onConfirm={() => {
          const key = confirmRemove?.key;
          setConfirmRemove(null);
          if (key) void remove(key);
        }}
        onClose={() => setConfirmRemove(null)}
      >
        PM stops watching <span className="text-ink2">{confirmRemove?.label}</span> and won’t index
        its changes anymore. Its already-indexed items are kept and stay findable, but marked
        “source unreachable” — they are never deleted. Add the folder again to resume.
      </ConfirmDialog>
    </div>
  );
}

/**
 * The per-folder subfolder picker: the whole folder is indexed, and unchecking a subfolder **excludes**
 * it (and its subtree). Reuses the shared {@link FolderPicker} with `rootIncluded` — local folders have
 * no "seed root" concept, so only excludes are ever produced. Saves auto (serialized, latest-wins) and
 * take effect on the folder's next **Sync** (already-indexed files under a newly-excluded folder go soft
 * then; the live watcher can't retroactively remove them).
 */
function LocalFolderExcludes({ folder, onSaved }: { folder: LocalFolder; onSaved: () => void }) {
  const [excluded, setExcluded] = useState<string[]>(folder.exclude);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const saveSeq = useRef(0);
  const saveChain = useRef<Promise<void>>(Promise.resolve());

  // A subfolder's id IS its root-relative path (what an exclude stores); `null` is the folder root.
  const loadChildren = useCallback(
    (parentId: string | null) =>
      listLocalSubfolders(folder.key, parentId).then((subs) =>
        subs.map((s) => ({ id: s.rel, name: s.name })),
      ),
    [folder.key],
  );

  const commit = (next: string[]) => {
    setExcluded(next);
    setError(null);
    setSaving(true);
    const seq = ++saveSeq.current;
    saveChain.current = saveChain.current
      .catch(() => {})
      .then(() => setLocalExcludes(folder.key, next))
      .then(() => {
        if (seq !== saveSeq.current) return; // a newer save owns the UI
        setSaving(false);
        onSaved();
      })
      .catch((e) => {
        if (seq !== saveSeq.current) return;
        setSaving(false);
        setError(String(e));
      });
  };

  return (
    <div className="mt-2">
      <FolderPicker
        loadChildren={loadChildren}
        selected={[]}
        excluded={excluded}
        rootIncluded
        onChange={(next) => commit(next.excluded)}
      />
      {error && <p className="mt-1 text-xs text-st-due">{error}</p>}
      <p className="mt-1 text-[0.6875rem] text-ink4">
        {excluded.length === 0 ? "Indexing the whole folder." : `${excluded.length} excluded`}
        {saving ? " · saving…" : ""} — applied on this folder’s next{" "}
        <span className="text-ink3">Sync</span>.
      </p>
    </div>
  );
}
