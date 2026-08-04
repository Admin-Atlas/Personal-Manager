// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useRef, useState } from "react";
import type { ReactNode } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import {
  connectDrive,
  connectOneDrive,
  disconnectDrive,
  disconnectOneDrive,
  driveStatus,
  driveSyncStatus,
  oneDriveStatus,
  oneDriveSyncStatus,
  onDriveSync,
  onOneDriveSync,
  reindexDrive,
  reindexOneDrive,
  stopDriveSync,
  stopOneDriveSync,
  syncDrive,
  syncOneDrive,
} from "../lib/ipc";
import type {
  DriveAccount,
  DriveStatus,
  DriveSyncState,
  OneDriveAccount,
  OneDriveStatus,
  OneDriveSyncState,
  SyncEvent,
} from "../lib/types";
import { useDetachedSync } from "../lib/useDetachedSync";
import { formatWhen } from "../lib/format";
import { Button, Callout, Collapsible, ConfirmDialog, SectionInfo } from "./ui";
import { SyncProgress } from "./SyncProgress";
import { SyncReport } from "./SyncReport";
import { ConnectorItemRow } from "./ConnectorItemRow";
import { SharedDrivesManager } from "./DriveSharedDrives";
import { OneDriveFolders } from "./OneDriveFolders";
import { GoogleOwnProjectConnect } from "./GoogleOwnProjectConnect";

/** Microsoft's app-access management page. Microsoft has no programmatic token revocation (unlike
 *  Google's RFC-7009 revoke), so disconnecting only forgets the local token — fully removing PM's
 *  access is done by the user here (L-3). */
const MICROSOFT_APPS_URL = "https://account.live.com/consent/Manage";

/** The two OAuth cloud-drive providers. Google Drive and OneDrive are near-identical index-only
 *  connectors — one provider-parameterised component serves both (the Calendar-connector pattern). */
type CloudProvider = "google" | "microsoft";

interface CloudDriveMeta {
  /** The service title shown in the connector. */
  title: string;
  /** The connector's help-anchor stem (`settings-drive` / `settings-onedrive`); the first-sync banner
   *  and post-sync report append `-firstsync` / `-report`. */
  help: string;
  /** The provider name in the connect button's "Waiting for …" state. */
  signInName: string;
  /** The provider name used in the "set up … above" hint. */
  signInHint: string;
  /** The noun in the not-configured hint ("connect a **Drive** account"). */
  accountNoun: string;
  connectLabel: string;
  disconnectTitle: string;
  /** The scope collapsible's title ("Shared drives & scope" / "Folders & scope"). */
  scopeTitle: string;
  /** The all-accounts progress-bar caption. */
  indexingAll: string;
  /** The "first sync indexes your **entire Drive**" noun phrase. */
  firstSyncWhole: string;
  blurb: ReactNode;
  emptyHint: ReactNode;
  needsFirstSync: ReactNode;
  status: () => Promise<DriveStatus | OneDriveStatus>;
  connect: () => Promise<unknown>;
  disconnect: (email: string) => Promise<void>;
  sync: (target: string | null) => Promise<unknown>;
  /** Re-index ONE account from scratch — forget its delta cursor, then sync. */
  reindex: (email: string) => Promise<unknown>;
  stop: () => Promise<unknown>;
  subscribe: (cb: (ev: SyncEvent) => void) => Promise<UnlistenFn>;
  syncStatus: () => Promise<DriveSyncState | OneDriveSyncState>;
  /** The per-account scope picker (shared drives for Google, folder tree for OneDrive). */
  scopePicker: (email: string, onSaved: () => void) => ReactNode;
  /** Google only: show the "Reconnect for Sheets" nudge on accounts without the Sheets scope. */
  sheets: boolean;
  /** Google only: offer the Advanced-Protection own-project connect path. */
  ownProject: boolean;
}

const CLOUD_DRIVE_META: Record<CloudProvider, CloudDriveMeta> = {
  google: {
    title: "Google Drive",
    help: "settings-drive",
    signInName: "Google",
    signInHint: "Google sign-in",
    accountNoun: "Drive",
    connectLabel: "Connect Google Drive",
    disconnectTitle: "Disconnect this Google Drive account?",
    scopeTitle: "Shared drives & scope",
    indexingAll: "Indexing your Drive",
    firstSyncWhole: "entire Drive",
    blurb: (
      <>
        Index your Drive files (read-only). Everything is <em>index-only</em> — a searchable pointer
        and a short summary; the full file stays in Drive and is fetched on demand. Each account
        indexes your personal <strong>My Drive</strong> by default; expand an account to add{" "}
        <strong>shared drives</strong> (folder-scoped by default).
      </>
    ),
    emptyHint: (
      <>
        You’ll be asked which Google account to use — connect your <strong>main</strong> one first;
        it heads the list. You can add more accounts afterwards, and each is indexed independently.
      </>
    ),
    needsFirstSync: (
      <>
        <span className="text-ink2">Choose what to index first.</span> Each account indexes your
        whole <strong>My Drive</strong> by default — expand{" "}
        <span className="text-ink3">Shared drives &amp; scope</span> to limit it to folders or add
        shared drives. When you’re ready, press <span className="text-ink2">Sync now</span> to start
        indexing — the first sync can take a while; later syncs only fetch what changed.
      </>
    ),
    status: driveStatus,
    connect: connectDrive,
    disconnect: disconnectDrive,
    sync: syncDrive,
    reindex: reindexDrive,
    stop: stopDriveSync,
    subscribe: onDriveSync,
    syncStatus: driveSyncStatus,
    scopePicker: (email, onSaved) => <SharedDrivesManager email={email} onSaved={onSaved} />,
    sheets: true,
    ownProject: true,
  },
  microsoft: {
    title: "OneDrive",
    help: "settings-onedrive",
    signInName: "Microsoft",
    signInHint: "Microsoft sign-in",
    accountNoun: "OneDrive",
    connectLabel: "Connect OneDrive",
    disconnectTitle: "Disconnect this OneDrive account?",
    scopeTitle: "Folders & scope",
    indexingAll: "Indexing your OneDrive",
    firstSyncWhole: "entire OneDrive",
    blurb: (
      <>
        Index your OneDrive files (read-only). Everything is <em>index-only</em> — a searchable
        pointer and a short summary; the full file stays in OneDrive and is fetched on demand. Each
        account indexes your whole OneDrive by default; expand an account to index just the{" "}
        <strong>folders you choose</strong>.
      </>
    ),
    emptyHint: (
      <>
        You’ll be asked which Microsoft account to use — connect your <strong>main</strong> one
        first; it heads the list. You can add more accounts afterwards, and each is indexed
        independently.
      </>
    ),
    needsFirstSync: (
      <>
        <span className="text-ink2">Choose what to index first.</span> Each account indexes your
        whole <strong>OneDrive</strong> by default — expand{" "}
        <span className="text-ink3">Folders &amp; scope</span> to limit it to just the folders you
        pick. When you’re ready, press <span className="text-ink2">Sync now</span> to start indexing
        — the first sync can take a while; later syncs only fetch what changed.
      </>
    ),
    status: oneDriveStatus,
    connect: connectOneDrive,
    disconnect: disconnectOneDrive,
    sync: syncOneDrive,
    reindex: reindexOneDrive,
    stop: stopOneDriveSync,
    subscribe: onOneDriveSync,
    syncStatus: oneDriveSyncStatus,
    scopePicker: (email, onSaved) => <OneDriveFolders email={email} onSaved={onSaved} />,
    sheets: false,
    ownProject: false,
  },
};

/**
 * **Cloud-drive connection** (Google Drive / OneDrive — read-only, index-only) under the Connectors
 * tab's Google / Microsoft groups. The two providers run the identical detached, single-flighted,
 * stop-able index-only sync (owned by {@link useDetachedSync}) and share the same account list, progress,
 * report, and stop UI; they differ only in a handful of labels and three affordances carried in
 * {@link CLOUD_DRIVE_META}: the per-account scope picker (shared drives vs folders), the Google-only
 * "Reconnect for Sheets" nudge, and the Google-only own-project (Advanced-Protection) connect path.
 *
 * The shared BYO OAuth client is set up once at the provider level (see {@link "./ConnectorsSettings"});
 * this component connects one or more accounts once that client is configured. Each connected account is
 * independent — its own sign-in, sync cursor, and indexed items. `refreshSignal` is bumped by the parent
 * group when the shared client is saved/cleared, so this refetches its status. `provider` is fixed for
 * the component's lifetime (the two groups render two separate instances).
 */
export function CloudDriveConnection({
  provider,
  refreshSignal = 0,
}: {
  provider: CloudProvider;
  refreshSignal?: number;
}) {
  const meta = CLOUD_DRIVE_META[provider];
  const [status, setStatus] = useState<DriveStatus | OneDriveStatus | null>(null);
  const [confirmEmail, setConfirmEmail] = useState<string | null>(null);
  // The account awaiting a "re-index everything?" confirmation. Its own state, not folded into
  // `confirmEmail`: the two dialogs say opposite things (one forgets an account, the other re-reads
  // it) and sharing one slot is how a confirm ends up firing the wrong action.
  const [confirmReindex, setConfirmReindex] = useState<string | null>(null);

  // The list refetch is called from the sync hook's "finished" event; a ref breaks the definition
  // cycle (the hook needs onSettled, `refresh` needs the hook's setError).
  const refreshRef = useRef<() => void>(() => {});
  const ds = useDetachedSync<DriveSyncState | OneDriveSyncState>({
    subscribe: meta.subscribe,
    fetchStatus: meta.syncStatus,
    targetOf: (s) => s.account,
    start: meta.sync,
    stop: meta.stop,
    onSettled: () => refreshRef.current(),
  });
  const { busy, error, setError, syncing, target: syncTarget, queued, report, progress } = ds;

  const refresh = useCallback(async () => {
    try {
      setStatus(await meta.status());
    } catch (e) {
      setError(String(e));
    }
  }, [meta, setError]);
  useEffect(() => {
    refreshRef.current = () => void refresh();
  }, [refresh]);

  // Refetch on mount, and whenever the parent group reports the shared client changed.
  useEffect(() => {
    void refresh();
  }, [refresh, refreshSignal]);

  const connect = () =>
    ds.run("connect", async () => {
      await meta.connect();
      await refresh();
      // No auto-sync: the account lands "not synced yet" so you can choose its scope first and then
      // start indexing yourself with "Sync now". The post-connect banner below points at that step.
    });

  const disconnect = (email: string) =>
    ds.run("disconnect", async () => {
      await meta.disconnect(email);
      await refresh();
    });

  // Re-index one account from scratch. Fire-and-forget like `ds.sync`: the backend forgets the
  // account's delta cursor and then runs an ordinary pass, so progress arrives on the same global
  // event stream and the bar behaves exactly as it does for Sync now. Not routed through `ds.run` —
  // that sets `busy`, which disables the whole connector, and this starts a detached sync rather
  // than a short blocking action.
  const reindex = (email: string) => {
    setError(null);
    void meta.reindex(email).catch((e) => setError(String(e)));
  };

  const configured = status?.oauth_client_configured ?? false;
  const accounts: (DriveAccount | OneDriveAccount)[] = status?.accounts ?? [];
  const anyBusy = busy != null || syncing;
  // The "first sync is slow" banner only makes sense while a *first* sync runs — the account being
  // indexed has never synced (a freshly-added account; an all-accounts follow-up counts if any is still
  // unsynced). It clears when indexing finishes and returns when a new account is added.
  const syncingAccount = accounts.find((a) => a.email === syncTarget);
  const firstSync =
    syncing &&
    (syncingAccount ? !syncingAccount.last_synced_at : accounts.some((a) => !a.last_synced_at));
  // A reachable account connected but never indexed — its scope chooser auto-expands and the banner
  // below nudges the user to pick a scope and press "Sync now" (we no longer auto-sync on connect).
  const needsFirstSync = !syncing && accounts.some((a) => a.state === "ok" && !a.last_synced_at);

  return (
    <div data-help={meta.help}>
      <span className="text-sm font-medium text-ink">{meta.title}</span>

      {!configured ? (
        <p className="mt-2 text-xs text-ink4">
          Set up <span className="text-ink2">{meta.signInHint}</span> above to connect a{" "}
          {meta.accountNoun} account.
        </p>
      ) : (
        <>
          {accounts.length > 0 && (
            <ul className="mt-3 divide-y divide-rule rounded-[var(--radius)] border border-border">
              {accounts.map((a) => (
                <li key={a.id} className="px-3 py-2">
                  <ConnectorItemRow
                    title={a.email}
                    reachable={a.state === "ok"}
                    // The only non-'ok' state the Drive connector ever writes is 'error', which means
                    // "a sync didn't finish, cursor held, retry next pass" — NOT that the account is
                    // gone. The default badge called that "unreachable", so one failed file out of
                    // ten thousand painted the whole account dead.
                    badgeLabel={a.state === "error" ? "sync failed" : "unreachable"}
                    detail={
                      a.state === "error" ? (
                        <p className="mt-0.5 text-xs text-ink4">
                          The last sync didn&rsquo;t finish. Press Sync now to retry, or narrow what
                          it covers below.
                        </p>
                      ) : undefined
                    }
                    meta={
                      <>
                        {a.indexed} indexed
                        {a.last_synced_at
                          ? ` · synced ${formatWhen(a.last_synced_at)}`
                          : " · not synced yet"}
                      </>
                    }
                    syncingThis={syncTarget === a.email}
                    queued={syncing && queued.has(a.email)}
                    // Sync stays clickable for accounts *not* currently syncing, so you can queue one
                    // mid-index; only the syncing row and in-flight connect/disconnect block it.
                    syncDisabled={syncTarget === a.email || busy != null}
                    onSync={() => ds.sync(a.email)}
                    onReindex={() => setConfirmReindex(a.email)}
                    // Any sync in flight, not just this row's: the backend refuses a re-index while
                    // one is running, because that pass ends by writing a fresh cursor and would
                    // undo the clear. The control must say the same thing the command would.
                    reindexDisabled={syncing || busy != null}
                    reindexBlockedReason={
                      syncing
                        ? "Available once the current sync finishes."
                        : "Available once the current action finishes."
                    }
                    actionLabel="Disconnect"
                    actionDisabled={anyBusy}
                    onAction={() => setConfirmEmail(a.email)}
                  >
                    {meta.sheets && "has_sheets_scope" in a && !a.has_sheets_scope && (
                      // A tinted callout, not a grey line with a tertiary button after it. This
                      // appears once the first index has finished, on a settings page nobody has a
                      // reason to revisit — so on the one visit where it IS on screen it has to read
                      // as something to act on. The same condition puts a dot on the Connectors tab
                      // and the sidebar's Settings row, so it can be found at all.
                      <Callout
                        tone="info"
                        body="ink"
                        className="mt-2 flex flex-wrap items-center justify-between gap-x-3 gap-y-2"
                      >
                        <div className="min-w-0 flex-1">
                          <p className="text-xs font-medium text-ink2">
                            Spreadsheets are indexed by name only
                          </p>
                          <p className="mt-0.5 text-xs text-ink3">
                            This account was connected before PM could read Sheets. Google
                            can&rsquo;t add a permission to a sign-in you&rsquo;ve already given, so
                            it takes one reconnect — nothing is re-indexed and nothing is lost.
                          </p>
                        </div>
                        {/* Reconnect re-runs consent, which requests the Sheets scope and unions it onto
                            the existing Drive grant (prompt=select_account → pick this email). */}
                        <Button
                          variant="secondary"
                          size="sm"
                          onClick={connect}
                          disabled={anyBusy}
                          className="shrink-0"
                        >
                          Reconnect for Sheets
                        </Button>
                      </Callout>
                    )}
                  </ConnectorItemRow>
                  {/* Not gated on `state === "ok"`. Hiding the scope picker the moment a sync
                      half-failed took away the one control that narrows scope away from whatever is
                      failing, and only a fully clean sync or a disconnect/reconnect brought it back.
                      CalendarConnection keeps its picker visible when not ok; Drive was the outlier. */}
                  <Collapsible
                    className="mt-2"
                    defaultOpen={!a.last_synced_at}
                    title={<span className="text-xs text-ink3">{meta.scopeTitle}</span>}
                  >
                    {meta.scopePicker(a.email, () => void refresh())}
                  </Collapsible>
                </li>
              ))}
            </ul>
          )}

          {accounts.length === 0 && <p className="mt-3 text-xs text-ink4">{meta.emptyHint}</p>}

          {needsFirstSync && (
            <div
              className="mt-3 rounded-[var(--radius)] px-3 py-2 text-xs text-ink3"
              style={{ background: "color-mix(in oklab, var(--st-look) 14%, transparent)" }}
              data-help={`${meta.help}-firstsync`}
            >
              {meta.needsFirstSync}
            </div>
          )}

          {/* An all-accounts sweep is owed after this pass. It belongs to no single row — and the
              backend CLEARS the specific accounts when an all-request subsumes them, which the
              15-minute poller does routinely — so it reads as one line rather than a badge on every
              row. Without it, a user who queued one account mid-sync and came back would see nothing
              at all, since the account they named is no longer what the queue holds. */}
          {syncing && ds.queuedAll && (
            <p className="mt-3 text-xs text-ink4">
              Every account is queued for another pass once this one finishes.
            </p>
          )}

          {syncing && progress && (
            <SyncProgress
              startedAt={ds.startedAt}
              processed={progress.processed}
              total={progress.total}
              label={syncTarget ? `Indexing ${syncTarget}` : meta.indexingAll}
              stopping={ds.stopping}
              confirmStop={ds.confirmStop}
              setConfirmStop={ds.setConfirmStop}
              onStop={ds.requestStop}
            />
          )}

          {/* The "first sync is slow" expectation-setter — contextual: between the progress bar and the
              add-account button only while a first sync runs; clears when indexing finishes. */}
          {firstSync && (
            <div
              className="mt-3 rounded-[var(--radius)] px-3 py-2 text-xs text-ink3"
              style={{ background: "color-mix(in oklab, var(--st-look) 14%, transparent)" }}
              data-help={`${meta.help}-firstsync`}
            >
              The <span className="text-ink2">first sync indexes your {meta.firstSyncWhole}</span> —
              it can take a while and use bandwidth. Later syncs only fetch what changed.
            </div>
          )}

          <div className="mt-3">
            {/* Gated on `busy` only, not `anyBusy` — adding another account stays available while a
                sync runs, so you can connect one you forgot mid-index. */}
            <Button
              variant={accounts.length === 0 ? "primary" : "secondary"}
              onClick={connect}
              disabled={busy != null}
            >
              {busy === "connect"
                ? `Waiting for ${meta.signInName}…`
                : accounts.length === 0
                  ? meta.connectLabel
                  : "Add another account"}
            </Button>
            {meta.ownProject && (
              <GoogleOwnProjectConnect
                service="drive"
                disabled={busy != null}
                onConnected={refresh}
              />
            )}
          </div>

          {!syncing && report && (
            <SyncReport
              report={report}
              helpId={`${meta.help}-report`}
              onDismiss={ds.dismissReport}
            />
          )}
        </>
      )}

      {error && (
        <Callout as="p" className="mt-2">
          {error}
        </Callout>
      )}

      {/* The connector's standing explanation, folded at the foot so the account list and the
          connect button sit together. The wrapper's `data-help={meta.help}` is already the
          help-mode anchor for this material, so no helpId here — it would duplicate the id. */}
      <SectionInfo title={`How ${meta.title} works`}>
        <p>{meta.blurb}</p>
      </SectionInfo>

      <ConfirmDialog
        open={confirmReindex != null}
        title={`Re-index this ${meta.accountNoun} account?`}
        confirmLabel="Re-index everything"
        onConfirm={() => {
          const email = confirmReindex;
          setConfirmReindex(null);
          if (email) reindex(email);
        }}
        onClose={() => setConfirmReindex(null)}
      >
        <p>
          PM will read every file in this account&rsquo;s scope again, instead of asking{" "}
          {meta.signInName} what has changed since last time. On a large account that takes a while
          and uses bandwidth.
        </p>
        {/* What it does NOT do matters as much as what it does — "re-index" reads like "start over",
            and someone with a filed library needs to know their work is safe before pressing it. */}
        <p className="mt-2">
          Nothing is deleted and nothing you have filed is lost. Files PM already has are recognised
          and left alone, so this is mostly listing requests rather than re-downloading your
          documents.
        </p>
        <p className="mt-2">
          Worth doing when a file&rsquo;s details look out of date, or you suspect the account and
          PM have drifted apart &mdash; a change feed only reports what it noticed at the time.
        </p>
      </ConfirmDialog>

      <ConfirmDialog
        open={confirmEmail != null}
        title={meta.disconnectTitle}
        danger
        confirmLabel="Disconnect"
        onConfirm={() => {
          const email = confirmEmail;
          setConfirmEmail(null);
          if (email) void disconnect(email);
        }}
        onClose={() => setConfirmEmail(null)}
      >
        <p>
          This forgets the account&rsquo;s sign-in. Its indexed items are kept and stay findable,
          but marked &ldquo;source unreachable&rdquo; until you reconnect — they are never deleted.
        </p>
        {/* Microsoft alone can't be revoked from inside the app, so PM disconnecting is only half
            the job. This caveat lives here rather than in the connector's SectionInfo: it's the one
            moment it's actionable, and a fold would have made it the only copy — the help registry
            doesn't carry it. */}
        {provider === "microsoft" && (
          <p className="mt-2">
            Microsoft can&rsquo;t revoke an app&rsquo;s access from within the app, so PM forgets it
            here but Microsoft still lists it. To finish removing it, manage app access at{" "}
            <a
              href={MICROSOFT_APPS_URL}
              target="_blank"
              rel="noreferrer"
              className="underline hover:text-ink2"
            >
              account.live.com
            </a>
            .
          </p>
        )}
      </ConfirmDialog>
    </div>
  );
}
