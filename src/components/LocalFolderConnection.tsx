// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useRef, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import {
  addLocalFolder,
  listLocalFolders,
  localFolderSyncStatus,
  onLocalChanged,
  onLocalSync,
  removeLocalFolder,
  stopLocalFolderSync,
  syncLocalFolder,
} from "../lib/ipc";
import type { LocalFolder, LocalSyncReport } from "../lib/types";
import { Button, ConfirmDialog } from "./ui";
import { IngestProgress } from "./IngestProgress";

/**
 * **Local folders** (index-only) — the filesystem connector (board card 6), under the Connectors tab.
 * Unlike the cloud connectors it has no provider and no sign-in: you point PM at a folder on this
 * machine and it indexes the ingestible files inside it. Every file is **index-only** — PM stores a
 * searchable pointer + a short summary, never the bytes; the file stays on disk and is read on demand.
 *
 * After a folder is added and synced once, a live filesystem **watcher** keeps it current in the
 * background (edits re-embed, renames/moves keep the same item, deletes go soft, unmount → unreachable),
 * so "Sync now" is mostly a manual catch-up. The watcher emits `local://changed` when it applies a
 * batch; we refetch the list on that so counts and state badges stay live without a manual refresh.
 *
 * The sync runs **detached** in the backend (it keeps going if you leave Settings) and single-flights,
 * exactly like the Drive/OneDrive connectors — so this view mirrors their progress/report/stop UI and
 * restores an in-flight sync's bar from the backend snapshot on (re)mount.
 */
export function LocalFolderConnection() {
  const [folders, setFolders] = useState<LocalFolder[]>([]);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [progress, setProgress] = useState<{ processed: number; total: number | null } | null>(
    null,
  );
  // The folder key being synced, or null for an all-folders pass.
  const [syncKey, setSyncKey] = useState<string | null>(null);
  // Folders whose "Sync now" was pressed while another sync was already running — the backend folds
  // them into a follow-up pass, so their row shows "Queued" until the current run ends.
  const [queued, setQueued] = useState<Set<string>>(new Set());
  const [report, setReport] = useState<LocalSyncReport | null>(null);
  const [stopping, setStopping] = useState(false);
  const [confirmStop, setConfirmStop] = useState(false);
  const [confirmRemove, setConfirmRemove] = useState<LocalFolder | null>(null);
  // Live mirror of "a sync is on screen", so a fire-and-forget "Sync now" can tell whether it owns the
  // visible bar without hijacking one already running (see `sync`).
  const syncingRef = useRef(false);

  const refresh = useCallback(async () => {
    try {
      setFolders(await listLocalFolders());
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  useEffect(() => {
    syncingRef.current = progress != null;
  }, [progress]);

  // Follow the detached sync's global progress events, and restore the bar from the backend snapshot if
  // a sync is already in flight when this view (re)mounts — so it never looks stalled. If it already
  // finished, show the last report so a returning user still sees the result.
  useEffect(() => {
    let mounted = true;
    const unlistenSync = onLocalSync((ev) => {
      if (!mounted) return;
      if (ev.type === "counted") setProgress({ processed: 0, total: ev.total });
      else if (ev.type === "item") setProgress({ processed: ev.processed, total: ev.total });
      else if (ev.type === "finished") {
        setProgress(null);
        setSyncKey(null);
        setQueued(new Set());
        setStopping(false);
        setReport(ev.report);
        void refresh();
      }
    });
    // The live watcher applied a batch of on-disk changes outside a manual sync — refetch so the
    // indexed counts and state badges reflect it. Cheap; the folder list is small.
    const unlistenChanged = onLocalChanged(() => {
      if (mounted) void refresh();
    });
    void localFolderSyncStatus()
      .then((s) => {
        if (!mounted) return;
        if (s.running) {
          setProgress({ processed: s.processed, total: s.total });
          setSyncKey(s.folder);
        } else if (s.last_report) {
          setReport(s.last_report);
        }
      })
      .catch(() => {});
    return () => {
      mounted = false;
      void unlistenSync.then((fn) => fn());
      void unlistenChanged.then((fn) => fn());
    };
  }, [refresh]);

  async function run(label: string, fn: () => Promise<void>) {
    setBusy(label);
    setError(null);
    try {
      await fn();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }

  // Start a background sync for one folder (or all). Fire-and-forget: progress arrives via the global
  // listener and the sync survives navigating away. The backend single-flights, so a click made while
  // one is already running is folded into a follow-up pass — only the call that *starts* a sync drives
  // the optimistic progress + error rollback, so it never hijacks a running bar.
  const sync = (key: string | null) => {
    setError(null);
    const startsIt = !syncingRef.current;
    if (startsIt) {
      setReport(null);
      setSyncKey(key);
      setQueued(new Set());
      setProgress({ processed: 0, total: null });
    } else if (key != null) {
      setQueued((q) => new Set(q).add(key));
    }
    void syncLocalFolder(key).catch((e) => {
      if (startsIt) {
        setError(String(e));
        setProgress(null);
        setSyncKey(null);
      }
    });
  };

  // Stop the running sync (after a confirm). The backend halts after the current file and keeps
  // everything indexed so far; the "finished" event (a `cancelled` report) clears the bar.
  const stop = () => {
    setStopping(true);
    void stopLocalFolderSync().catch(() => setStopping(false));
  };

  // Native folder picker → register → index it right away. There's no scope to choose (unlike Drive's
  // My-Drive-vs-folders), so adding a folder means "index this" — we kick off its first sync at once.
  const add = () =>
    run("add", async () => {
      const selected = await open({ directory: true });
      if (!selected) return;
      const key = await addLocalFolder(selected as string);
      await refresh();
      sync(key);
    });

  const remove = (key: string) =>
    run("remove", async () => {
      await removeLocalFolder(key);
      await refresh();
    });

  const syncing = progress != null;
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
              <FolderRow
                folder={f}
                syncingThis={syncKey === f.key}
                queued={syncing && queued.has(f.key)}
                // Sync stays clickable for folders *not* currently syncing, so one can be queued
                // mid-index; only the syncing row and in-flight add/remove block it.
                syncDisabled={syncKey === f.key || busy != null}
                removeDisabled={anyBusy}
                onSync={() => sync(f.key)}
                onRemove={() => setConfirmRemove(f)}
              />
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
        <div className="mt-3">
          <IngestProgress
            processed={progress.processed}
            total={progress.total}
            label={syncKey ? "Indexing folder" : "Indexing your folders"}
          />
          <p className="mt-1 text-xs text-ink4">
            Indexing keeps running in the background — you can leave this page and come back later;
            we’ll keep working.
          </p>
          <div
            className="mt-2 rounded-[var(--radius)] px-3 py-2 text-xs text-ink3"
            style={{ background: "color-mix(in oklab, var(--st-due) 12%, transparent)" }}
          >
            Changed your mind?{" "}
            <span className="text-ink2">Stopping keeps everything indexed so far</span> — those
            files stay searchable; the rest just won’t be indexed until you sync again.
            <div className="mt-2">
              <Button
                variant="tertiary"
                onClick={() => setConfirmStop(true)}
                disabled={stopping}
                className="px-2 py-1 text-xs hover:text-st-due disabled:opacity-40"
              >
                {stopping ? "Stopping…" : "Stop indexing"}
              </Button>
            </div>
          </div>
        </div>
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

      {!syncing && report && <SyncReport report={report} onDismiss={() => setReport(null)} />}

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

      <ConfirmDialog
        open={confirmStop}
        title="Stop indexing?"
        danger
        confirmLabel="Stop indexing"
        onConfirm={() => {
          setConfirmStop(false);
          stop();
        }}
        onClose={() => setConfirmStop(false)}
      >
        Everything indexed so far is kept and stays searchable — only the files not yet reached will
        be left out, and a later sync picks them up. You can resume any time with “Sync now”.
      </ConfirmDialog>
    </div>
  );
}

function FolderRow({
  folder,
  syncingThis,
  queued,
  syncDisabled,
  removeDisabled,
  onSync,
  onRemove,
}: {
  folder: LocalFolder;
  syncingThis: boolean;
  queued: boolean;
  syncDisabled: boolean;
  removeDisabled: boolean;
  onSync: () => void;
  onRemove: () => void;
}) {
  // "unreachable" is the registry state (a failed sync / removed root); `present:false` is a live
  // check that the path isn't a readable directory right now — surface either as a warning badge.
  const missing = folder.state !== "ok" || !folder.present;
  return (
    <div className="flex items-center justify-between gap-2">
      <div className="min-w-0">
        <div className="flex items-center gap-2">
          <span className="truncate text-sm text-ink">{folder.label}</span>
          {missing ? (
            <span className="shrink-0 text-[10px] uppercase tracking-wide text-st-due">
              {folder.present ? "unreachable" : "not found"}
            </span>
          ) : (
            <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-[var(--st-quick)]" />
          )}
        </div>
        <p className="mt-0.5 truncate text-xs text-ink4" title={folder.path}>
          {folder.path}
        </p>
        <p className="mt-0.5 truncate text-xs text-ink4">
          {folder.indexed} indexed
          {folder.last_synced_at
            ? ` · synced ${formatWhen(folder.last_synced_at)}`
            : " · not synced yet"}
        </p>
      </div>
      <div className="flex shrink-0 items-center gap-1">
        <Button
          onClick={onSync}
          disabled={syncDisabled}
          className="px-2 py-1 text-xs disabled:opacity-40"
        >
          {syncingThis ? "Syncing…" : queued ? "Queued" : "Sync now"}
        </Button>
        <Button
          variant="tertiary"
          onClick={onRemove}
          disabled={removeDisabled}
          className="px-2 py-1 text-xs hover:text-st-due"
        >
          Remove
        </Button>
      </div>
    </div>
  );
}

function formatWhen(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
}

/** The post-sync summary: how many files were indexed, and an expandable list of any that couldn't
 *  be (unsupported types, read errors) so the user knows exactly what was left out. Mirrors the
 *  Drive/OneDrive report — same `LocalSyncReport` shape. */
function SyncReport({ report, onDismiss }: { report: LocalSyncReport; onDismiss: () => void }) {
  const [showIssues, setShowIssues] = useState(false);
  const touched = report.indexed + report.updated + report.removed;
  const issueCount = report.issues.length;
  return (
    <div
      className="mt-3 rounded-[var(--radius)] border border-border p-3"
      data-help="settings-local-report"
    >
      <div className="flex items-start justify-between gap-2">
        <div className="text-xs text-ink2">
          {report.cancelled ? (
            <span className="font-medium text-ink">Indexing stopped.</span>
          ) : (
            <span className="font-medium text-ink">Sync complete.</span>
          )}{" "}
          <span className="text-ink3">
            Indexed {report.indexed} · updated {report.updated} · removed {report.removed}
            {touched === 0 && " · nothing new"}.
          </span>
          {report.cancelled && (
            <span className="text-ink4">
              {" "}
              Everything indexed so far is kept — sync again to finish.
            </span>
          )}
        </div>
        <button
          type="button"
          onClick={onDismiss}
          aria-label="Dismiss summary"
          className="shrink-0 text-ink4 hover:text-ink2"
        >
          ×
        </button>
      </div>

      {report.issues.length > 0 && (
        <div className="mt-2">
          <button
            type="button"
            onClick={() => setShowIssues((v) => !v)}
            className="font-mono text-[10px] uppercase tracking-wide text-ink3 hover:text-ink"
          >
            {showIssues ? "▾" : "▸"} {issueCount}
            {report.issues_truncated ? "+" : ""} file
            {issueCount === 1 && !report.issues_truncated ? "" : "s"} not indexed
          </button>
          {showIssues && (
            <ul className="mt-1.5 max-h-40 space-y-1 overflow-auto">
              {report.issues.map((iss, i) => (
                <li key={i} className="text-[11px] leading-tight">
                  <span className="text-ink2">{iss.name}</span>
                  <span className="text-ink4"> — {iss.reason}</span>
                </li>
              ))}
            </ul>
          )}
        </div>
      )}

      <p className="mt-2 text-[11px] text-ink4">
        Indexed files are searchable and appear in Documents.
      </p>
    </div>
  );
}
