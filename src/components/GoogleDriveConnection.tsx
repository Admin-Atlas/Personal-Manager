// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useRef, useState } from "react";
import {
  connectDrive,
  disconnectDrive,
  driveStatus,
  driveSyncStatus,
  onDriveSync,
  stopDriveSync,
  syncDrive,
} from "../lib/ipc";
import type { DriveAccount, DriveStatus, DriveSyncReport } from "../lib/types";
import { Button, Collapsible, ConfirmDialog } from "./ui";
import { IngestProgress } from "./IngestProgress";
import { GoogleCredentialBlock } from "./GoogleCredentialBlock";
import { SharedDrivesManager } from "./DriveSharedDrives";

/**
 * **Google Drive** (read-only, index-only) — the first cloud-API connector (board card 4A), under
 * the Connectors tab's Drive section. Reuses the shared {@link GoogleCredentialBlock} for the BYO
 * Google client, then connects one or more Drive accounts. Each connected account is **independent**
 * — its own sign-in, sync, and indexed items.
 *
 * Every file is **index-only**: PM stores a searchable pointer + a short summary, never the bytes;
 * the full file stays in Drive and is fetched on demand. The first sync walks the whole Drive (the
 * banner warns it can be slow); later syncs apply only what changed.
 */
export function GoogleDriveConnection() {
  const [status, setStatus] = useState<DriveStatus | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [progress, setProgress] = useState<{ processed: number; total: number | null } | null>(
    null,
  );
  const [syncAccount, setSyncAccount] = useState<string | null>(null);
  const [report, setReport] = useState<DriveSyncReport | null>(null);
  const [stopping, setStopping] = useState(false);
  const [confirmStop, setConfirmStop] = useState(false);
  const [confirmEmail, setConfirmEmail] = useState<string | null>(null);
  // Live mirror of "a sync is on screen", so a callback created in an earlier render (the connect
  // tail, which may fire mid-sync) can tell whether it owns the visible progress without hijacking
  // a running sync's bar.
  const syncingRef = useRef(false);

  const refresh = useCallback(async () => {
    try {
      setStatus(await driveStatus());
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  // Mirror "is a sync on screen" into a ref for callbacks that outlive their render (see `sync`).
  useEffect(() => {
    syncingRef.current = progress != null;
  }, [progress]);

  // The sync runs detached in the backend (it keeps going if you leave Settings). Follow its global
  // progress events, and — if a sync is already in flight when this view (re)mounts — restore the bar
  // from the backend snapshot so it never looks stalled or stopped. If it already finished, show the
  // last report so a returning user still sees the result.
  useEffect(() => {
    let mounted = true;
    const unlisten = onDriveSync((ev) => {
      if (!mounted) return;
      if (ev.type === "counted") setProgress({ processed: 0, total: ev.total });
      else if (ev.type === "item") setProgress({ processed: ev.processed, total: ev.total });
      else if (ev.type === "finished") {
        setProgress(null);
        setSyncAccount(null);
        setStopping(false);
        setReport(ev.report);
        void refresh();
      }
    });
    void driveSyncStatus()
      .then((s) => {
        if (!mounted) return;
        if (s.running) {
          setProgress({ processed: s.processed, total: s.total });
          setSyncAccount(s.account);
        } else if (s.last_report) {
          setReport(s.last_report);
        }
      })
      .catch(() => {});
    return () => {
      mounted = false;
      void unlisten.then((fn) => fn());
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

  // Start a background sync for one account (or all). Fire-and-forget: progress arrives via the
  // global event listener above, and the sync survives navigating away. The backend single-flights
  // syncs, so a request made while one is already running (e.g. the connect tail after adding an
  // account mid-index) is folded into a follow-up pass — we must not hijack the running bar, so only
  // the call that *starts* a sync drives the optimistic progress + error rollback.
  const sync = (email: string | null) => {
    setError(null);
    const startsIt = !syncingRef.current;
    if (startsIt) {
      setReport(null);
      setSyncAccount(email);
      setProgress({ processed: 0, total: null });
    }
    void syncDrive(email).catch((e) => {
      if (startsIt) {
        setError(String(e));
        setProgress(null);
        setSyncAccount(null);
      }
    });
  };

  // Stop the running sync (after a confirm, so an accidental click doesn't halt a long index). The
  // backend halts after the current file and keeps everything indexed so far; the "finished" event
  // (with a `cancelled` report) clears the bar.
  const stop = () => {
    setStopping(true);
    void stopDriveSync().catch(() => setStopping(false));
  };

  const connect = () =>
    run("connect", async () => {
      const account = await connectDrive();
      await refresh();
      // Kick off the (slow) first sync in the background — the banner above sets the expectation.
      sync(account.email);
    });

  const disconnect = (email: string) =>
    run("disconnect", async () => {
      await disconnectDrive(email);
      await refresh();
    });

  const configured = status?.oauth_client_configured ?? false;
  const accounts = status?.accounts ?? [];
  const syncing = progress != null;
  const anyBusy = busy != null || syncing;

  return (
    <div data-help="settings-drive">
      <span className="text-sm font-medium text-ink">Google Drive</span>
      <p className="mt-1 text-xs text-ink4">
        Index your Drive files (read-only). Everything is <em>index-only</em> — a searchable pointer
        and a short summary; the full file stays in Drive and is fetched on demand. Each account
        indexes your personal <strong>My Drive</strong> by default; expand an account to add{" "}
        <strong>shared drives</strong> (folder-scoped by default).
      </p>

      {!configured ? (
        <div className="mt-2">
          <GoogleCredentialBlock configured={false} onChange={refresh} />
        </div>
      ) : (
        <>
          <div
            className="mt-2 rounded-[var(--radius)] px-3 py-2 text-xs text-ink3"
            style={{ background: "color-mix(in oklab, var(--st-look) 14%, transparent)" }}
            data-help="settings-drive-firstsync"
          >
            The <span className="text-ink2">first sync indexes your entire Drive</span> — it can
            take a while and use bandwidth. Later syncs only fetch what changed.
          </div>

          {accounts.length > 0 && (
            <ul className="mt-3 divide-y divide-rule rounded-[var(--radius)] border border-border">
              {accounts.map((a) => (
                <li key={a.id} className="px-3 py-2">
                  <AccountRow
                    account={a}
                    busy={anyBusy}
                    syncingThis={syncAccount === a.email}
                    onSync={() => sync(a.email)}
                    onDisconnect={() => setConfirmEmail(a.email)}
                  />
                  {a.state === "ok" && (
                    <Collapsible
                      className="mt-2"
                      defaultOpen={false}
                      title={<span className="text-xs text-ink3">Shared drives & scope</span>}
                    >
                      <SharedDrivesManager email={a.email} busy={anyBusy} onSaved={refresh} />
                    </Collapsible>
                  )}
                </li>
              ))}
            </ul>
          )}

          <div className="mt-3">
            {/* Deliberately gated on `busy` only, not `anyBusy` — adding another account stays
                available while a sync runs, so you can connect one you forgot mid-index. */}
            <Button
              variant={accounts.length === 0 ? "primary" : "secondary"}
              onClick={connect}
              disabled={busy != null}
              className="disabled:opacity-40"
            >
              {busy === "connect"
                ? "Waiting for Google…"
                : accounts.length === 0
                  ? "Connect Google Drive"
                  : "Add another account"}
            </Button>
          </div>

          {syncing && progress && (
            <div className="mt-3">
              <IngestProgress
                processed={progress.processed}
                total={progress.total}
                label={syncAccount ? `Indexing ${syncAccount}` : "Indexing your Drive"}
              />
              <p className="mt-1 text-xs text-ink4">
                Indexing keeps running in the background — you can leave this page and come back
                later; we’ll keep working.
              </p>
              <div
                className="mt-2 rounded-[var(--radius)] px-3 py-2 text-xs text-ink3"
                style={{ background: "color-mix(in oklab, var(--st-due) 12%, transparent)" }}
              >
                Changed your mind about the size of this?{" "}
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

          {!syncing && report && <SyncReport report={report} onDismiss={() => setReport(null)} />}
        </>
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
        open={confirmEmail != null}
        title="Disconnect this Google Drive account?"
        danger
        confirmLabel="Disconnect"
        onConfirm={() => {
          const email = confirmEmail;
          setConfirmEmail(null);
          if (email) void disconnect(email);
        }}
        onClose={() => setConfirmEmail(null)}
      >
        This forgets the account's sign-in. Its indexed items are kept and stay findable, but marked
        “source unreachable” until you reconnect — they are never deleted.
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
        be left out, and a later sync picks them up where this one stopped. You can resume any time
        with “Sync now”.
      </ConfirmDialog>
    </div>
  );
}

function AccountRow({
  account,
  busy,
  syncingThis,
  onSync,
  onDisconnect,
}: {
  account: DriveAccount;
  busy: boolean;
  syncingThis: boolean;
  onSync: () => void;
  onDisconnect: () => void;
}) {
  const unreachable = account.state !== "ok";
  return (
    <div className="flex items-center justify-between gap-2">
      <div className="min-w-0">
        <div className="flex items-center gap-2">
          <span className="truncate text-sm text-ink">{account.email}</span>
          {unreachable ? (
            <span className="shrink-0 text-[10px] uppercase tracking-wide text-st-due">
              unreachable
            </span>
          ) : (
            <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-[var(--st-quick)]" />
          )}
        </div>
        <p className="mt-0.5 truncate text-xs text-ink4">
          {account.indexed} indexed
          {account.last_synced_at
            ? ` · synced ${formatWhen(account.last_synced_at)}`
            : " · not synced yet"}
        </p>
      </div>
      <div className="flex shrink-0 items-center gap-1">
        <Button onClick={onSync} disabled={busy} className="px-2 py-1 text-xs disabled:opacity-40">
          {syncingThis ? "Syncing…" : "Sync now"}
        </Button>
        <Button
          variant="tertiary"
          onClick={onDisconnect}
          disabled={busy}
          className="px-2 py-1 text-xs hover:text-st-due"
        >
          Disconnect
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
 *  be (unsupported types, fetch errors) so the user knows exactly what was left out. */
function SyncReport({ report, onDismiss }: { report: DriveSyncReport; onDismiss: () => void }) {
  const [showIssues, setShowIssues] = useState(false);
  const touched = report.indexed + report.updated + report.removed;
  const issueCount = report.issues.length;
  return (
    <div
      className="mt-3 rounded-[var(--radius)] border border-border p-3"
      data-help="settings-drive-report"
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
