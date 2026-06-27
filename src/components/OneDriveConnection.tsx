// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useRef, useState } from "react";
import {
  connectOneDrive,
  disconnectOneDrive,
  oneDriveStatus,
  oneDriveSyncStatus,
  onOneDriveSync,
  stopOneDriveSync,
  syncOneDrive,
} from "../lib/ipc";
import type { OneDriveAccount, OneDriveStatus, OneDriveSyncReport } from "../lib/types";
import { Button, Collapsible, ConfirmDialog } from "./ui";
import { IngestProgress } from "./IngestProgress";
import { MicrosoftCredentialBlock } from "./MicrosoftCredentialBlock";
import { OneDriveFolders } from "./OneDriveFolders";

/**
 * **Microsoft OneDrive** (read-only, index-only) — the second cloud-API connector (board card 4B), a
 * mirror of {@link "./GoogleDriveConnection"}. Reuses the shared {@link MicrosoftCredentialBlock} for
 * the BYO Microsoft client, then connects one or more OneDrive accounts. Each connected account is
 * **independent** — its own sign-in, sync, and indexed items.
 *
 * Every file is **index-only**: PM stores a searchable pointer + a short summary, never the bytes; the
 * full file stays in OneDrive and is fetched on demand. The first sync walks the whole drive (the
 * banner warns it can be slow); later syncs apply only what changed (the Graph delta query). Expand an
 * account to index just the folders you choose.
 */
export function OneDriveConnection() {
  const [status, setStatus] = useState<OneDriveStatus | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [progress, setProgress] = useState<{ processed: number; total: number | null } | null>(
    null,
  );
  const [syncAccount, setSyncAccount] = useState<string | null>(null);
  const [report, setReport] = useState<OneDriveSyncReport | null>(null);
  const [stopping, setStopping] = useState(false);
  const [confirmStop, setConfirmStop] = useState(false);
  const [confirmEmail, setConfirmEmail] = useState<string | null>(null);
  // Live mirror of "a sync is on screen", so the connect tail (which may fire mid-sync) can tell
  // whether it owns the visible progress without hijacking a running sync's bar.
  const syncingRef = useRef(false);

  const refresh = useCallback(async () => {
    try {
      setStatus(await oneDriveStatus());
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  useEffect(() => {
    syncingRef.current = progress != null;
  }, [progress]);

  // The sync runs detached in the backend. Follow its global progress events, and — if a sync is
  // already in flight when this view (re)mounts — restore the bar from the backend snapshot.
  useEffect(() => {
    let mounted = true;
    const unlisten = onOneDriveSync((ev) => {
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
    void oneDriveSyncStatus()
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

  // Start a background sync for one account (or all). Fire-and-forget: progress arrives via the global
  // event listener above. Only the call that *starts* a sync drives the optimistic progress + error
  // rollback (the backend single-flights, folding a request made mid-sync into a follow-up pass).
  const sync = (email: string | null) => {
    setError(null);
    const startsIt = !syncingRef.current;
    if (startsIt) {
      setReport(null);
      setSyncAccount(email);
      setProgress({ processed: 0, total: null });
    }
    void syncOneDrive(email).catch((e) => {
      if (startsIt) {
        setError(String(e));
        setProgress(null);
        setSyncAccount(null);
      }
    });
  };

  const stop = () => {
    setStopping(true);
    void stopOneDriveSync().catch(() => setStopping(false));
  };

  const connect = () =>
    run("connect", async () => {
      const account = await connectOneDrive();
      await refresh();
      sync(account.email);
    });

  const disconnect = (email: string) =>
    run("disconnect", async () => {
      await disconnectOneDrive(email);
      await refresh();
    });

  const configured = status?.oauth_client_configured ?? false;
  const accounts = status?.accounts ?? [];
  const syncing = progress != null;
  const anyBusy = busy != null || syncing;

  return (
    <div data-help="settings-onedrive">
      <span className="text-sm font-medium text-ink">OneDrive</span>
      <p className="mt-1 text-xs text-ink4">
        Index your OneDrive files (read-only). Everything is <em>index-only</em> — a searchable
        pointer and a short summary; the full file stays in OneDrive and is fetched on demand. Each
        account indexes your whole OneDrive by default; expand an account to index just the{" "}
        <strong>folders you choose</strong>.
      </p>

      {!configured ? (
        <div className="mt-2">
          <MicrosoftCredentialBlock configured={false} onChange={refresh} />
        </div>
      ) : (
        <>
          <div
            className="mt-2 rounded-[var(--radius)] px-3 py-2 text-xs text-ink3"
            style={{ background: "color-mix(in oklab, var(--st-look) 14%, transparent)" }}
            data-help="settings-onedrive-firstsync"
          >
            The <span className="text-ink2">first sync indexes your entire OneDrive</span> — it can
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
                      title={<span className="text-xs text-ink3">Folders &amp; scope</span>}
                    >
                      <OneDriveFolders email={a.email} busy={anyBusy} onSaved={refresh} />
                    </Collapsible>
                  )}
                </li>
              ))}
            </ul>
          )}

          <div className="mt-3">
            {/* Gated on `busy` only, not `anyBusy` — adding another account stays available while a
                sync runs. */}
            <Button
              variant={accounts.length === 0 ? "primary" : "secondary"}
              onClick={connect}
              disabled={busy != null}
              className="disabled:opacity-40"
            >
              {busy === "connect"
                ? "Waiting for Microsoft…"
                : accounts.length === 0
                  ? "Connect OneDrive"
                  : "Add another account"}
            </Button>
          </div>

          {syncing && progress && (
            <div className="mt-3">
              <IngestProgress
                processed={progress.processed}
                total={progress.total}
                label={syncAccount ? `Indexing ${syncAccount}` : "Indexing your OneDrive"}
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
        title="Disconnect this OneDrive account?"
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
  account: OneDriveAccount;
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
function SyncReport({ report, onDismiss }: { report: OneDriveSyncReport; onDismiss: () => void }) {
  const [showIssues, setShowIssues] = useState(false);
  const touched = report.indexed + report.updated + report.removed;
  const issueCount = report.issues.length;
  return (
    <div
      className="mt-3 rounded-[var(--radius)] border border-border p-3"
      data-help="settings-onedrive-report"
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
