// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useState } from "react";
import {
  calendarOverview,
  connectGoogleCalendarAccount,
  connectOutlookCalendar,
  disconnectGoogleCalendarAccount,
  disconnectOutlookCalendar,
  setCalendarSelected,
  syncCalendar,
} from "../lib/ipc";
import type { Calendar, CalendarAccount, CalendarOverview } from "../lib/types";
import { useDevMode } from "../lib/capabilities";
import { Button, ConfirmDialog, Skeleton } from "./ui";
import { DevPanel } from "./dev/DevPanel";
import { GoogleOwnProjectConnect } from "./GoogleOwnProjectConnect";

/** The two read-only OAuth calendar providers. Apple has no desktop OAuth, so it stays a subscription
 *  (see {@link "./IcsFeedSubscription"}). */
type Provider = "google" | "microsoft";

const PROVIDER_META: Record<
  Provider,
  {
    /** The service title shown in the connector. */
    label: string;
    /** The provider name used in the "set up sign-in above" hint. */
    sign_in: string;
    blurb: string;
    connect: () => Promise<CalendarAccount>;
    disconnect: (email: string) => Promise<void>;
  }
> = {
  google: {
    label: "Google Calendar",
    sign_in: "Google sign-in",
    blurb:
      "Read-only sign-in with your own Google client. Connect one or more Google accounts; PM powers your agenda, schedule questions in chat, and the “Due soon” status when an event names a project.",
    connect: connectGoogleCalendarAccount,
    disconnect: disconnectGoogleCalendarAccount,
  },
  microsoft: {
    label: "Outlook Calendar",
    sign_in: "Microsoft sign-in",
    blurb:
      "Read-only sign-in with your Microsoft 365 / Outlook account. Connect one or more accounts; PM powers your agenda, schedule questions in chat, and the “Due soon” status when an event names a project.",
    connect: connectOutlookCalendar,
    disconnect: disconnectOutlookCalendar,
  },
};

/**
 * **Calendar connection** (read-only OAuth) — the per-provider account + calendar manager under the
 * Connectors tab's Google / Microsoft groups. Google Calendar and Outlook are near-identical (the only
 * differences are the connect/disconnect commands and a few labels), so one provider-parameterised
 * component serves both rather than two duplicated files.
 *
 * The shared, provider-level BYO OAuth client is set up once at the group level (see
 * {@link "./ConnectorsSettings"}). Once it's configured, this offers Connect → browser, a per-account
 * calendar picker, Sync, and Disconnect. **Multi-account:** connect several accounts of the same
 * provider; each is independent. `refreshSignal` is bumped by the parent group when the shared client
 * is saved/cleared, so this refetches `calendar_overview`.
 */
export function CalendarConnection({
  provider,
  refreshSignal = 0,
}: {
  provider: Provider;
  refreshSignal?: number;
}) {
  const meta = PROVIDER_META[provider];
  const { devMode } = useDevMode();
  const [overview, setOverview] = useState<CalendarOverview | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmEmail, setConfirmEmail] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setOverview(await calendarOverview());
    } catch (e) {
      setError(String(e));
    }
  }, []);

  // Refetch on mount, and whenever the parent group reports the shared client changed.
  useEffect(() => {
    void refresh();
  }, [refresh, refreshSignal]);

  async function run(label: string, fn: () => Promise<void>) {
    setBusy(label);
    setError(null);
    setNote(null);
    try {
      await fn();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }

  const configured =
    provider === "google"
      ? (overview?.google_client_configured ?? false)
      : (overview?.microsoft_client_configured ?? false);
  const accounts = (overview?.accounts ?? []).filter((a) => a.provider === provider);
  const calendarsFor = (sourceId: string) =>
    (overview?.calendars ?? [])
      .filter((c) => c.source_id === sourceId)
      .sort((a, b) => Number(b.is_primary) - Number(a.is_primary) || a.name.localeCompare(b.name));

  // Post-connect: refresh the account list, kick a first sync, and report the count. Shared by the
  // normal connect and the own-project (Advanced-Protection) connect path below.
  const afterConnect = async () => {
    await refresh();
    const n = await syncCalendar().catch(() => 0);
    setNote(`Connected. Synced ${n} event${n === 1 ? "" : "s"}.`);
    await refresh();
  };

  const connect = () =>
    run("connect", async () => {
      await meta.connect();
      await afterConnect();
    });

  const disconnect = (email: string) =>
    run("disconnect", async () => {
      await meta.disconnect(email);
      await refresh();
    });

  const toggle = (cal: Calendar, on: boolean) =>
    run("select", async () => {
      // Optimistic flip, rolled back by a refresh if the backend rejects it.
      setOverview((o) =>
        o
          ? {
              ...o,
              calendars: o.calendars.map((c) => (c.id === cal.id ? { ...c, selected: on } : c)),
            }
          : o,
      );
      try {
        await setCalendarSelected(cal.id, on);
      } catch (e) {
        await refresh();
        throw e;
      }
      const n = await syncCalendar().catch(() => 0);
      setNote(`Synced ${n} event${n === 1 ? "" : "s"}.`);
      await refresh();
    });

  const sync = () =>
    run("sync", async () => {
      const n = await syncCalendar();
      setNote(`Synced ${n} event${n === 1 ? "" : "s"}.`);
      await refresh();
    });

  // A connected Google account can still 403 because the Calendar API isn't enabled in the user's
  // Cloud project — surface that as an actionable enable-link rather than a raw wall of text.
  const apiDisabled = provider === "google" ? calendarApiDisabled(error) : null;

  return (
    <div data-help={`settings-calendar-${provider}`}>
      <span className="text-sm font-medium text-ink">{meta.label}</span>
      <p className="mt-1 text-xs text-ink4">{meta.blurb}</p>

      {!configured && (
        <p className="mt-2 text-xs text-ink4">
          Set up <span className="text-ink2">{meta.sign_in}</span> above to connect your calendar.
        </p>
      )}

      {configured && (
        <>
          {overview == null ? (
            <div className="mt-3 flex flex-col gap-1.5">
              {Array.from({ length: 2 }).map((_, i) => (
                <Skeleton key={i} className="h-7 w-full" />
              ))}
            </div>
          ) : accounts.length === 0 ? (
            <p className="mt-3 text-xs text-ink4">
              You’ll be asked which account to use — connect your <strong>main</strong> one first;
              it heads the list. You can add more accounts afterwards, and each is independent.
            </p>
          ) : (
            <ul className="mt-3 divide-y divide-rule rounded-[var(--radius)] border border-border">
              {accounts.map((a) => (
                <li key={a.id} className="px-3 py-2">
                  <AccountBlock
                    account={a}
                    calendars={calendarsFor(a.id)}
                    busy={busy != null}
                    onToggle={toggle}
                    onDisconnect={() => setConfirmEmail(a.email)}
                  />
                </li>
              ))}
            </ul>
          )}

          <div className="mt-3 flex items-center justify-between gap-2">
            <p className="text-xs text-ink4">
              {overview?.last_sync
                ? `Last synced ${formatWhen(overview.last_sync)} · ${overview.window_days} days ahead`
                : `Not synced yet · ${overview?.window_days ?? 21} days ahead`}
            </p>
            {accounts.length > 0 && (
              <Button
                onClick={sync}
                disabled={busy != null}
                className="px-2 py-1 text-xs disabled:opacity-40"
              >
                {busy === "sync" || busy === "select" ? "Syncing…" : "Sync now"}
              </Button>
            )}
          </div>

          <div className="mt-2">
            <Button
              variant={accounts.length === 0 ? "primary" : "secondary"}
              onClick={connect}
              disabled={busy != null}
              className="disabled:opacity-40"
            >
              {busy === "connect"
                ? `Waiting for ${meta.sign_in.split(" ")[0]}…`
                : accounts.length === 0
                  ? `Connect ${meta.label}`
                  : "Add another account"}
            </Button>
            {provider === "google" && (
              <GoogleOwnProjectConnect
                service="calendar"
                disabled={busy != null}
                onConnected={afterConnect}
              />
            )}
          </div>
        </>
      )}

      {note && <p className="mt-2 text-xs text-st-quick">{note}</p>}
      {error &&
        (apiDisabled ? (
          <div
            className="mt-2 rounded-[var(--radius)] px-3 py-2 text-xs text-st-due"
            style={{ background: "color-mix(in oklab, var(--st-due) 15%, transparent)" }}
          >
            <p>
              Your Google account is connected, but the{" "}
              <span className="font-medium">Google Calendar API</span> isn&apos;t enabled in your
              Google Cloud project yet.
            </p>
            <p className="mt-1">
              <a
                href={apiDisabled.enableUrl}
                target="_blank"
                rel="noreferrer"
                className="text-accent-text underline hover:brightness-110"
              >
                Enable the Google Calendar API
              </a>{" "}
              (with your project selected), give it a minute, then Sync again.
            </p>
          </div>
        ) : (
          <p
            className="mt-2 rounded-[var(--radius)] px-3 py-2 text-xs text-st-due"
            style={{ background: "color-mix(in oklab, var(--st-due) 15%, transparent)" }}
          >
            {error}
          </p>
        ))}

      {devMode && overview && (
        <DevPanel
          title={`Calendar sync state (${provider})`}
          helpId="dev-calendar"
          subtitle="Connected accounts + per-source state. No tokens or feed URLs (keychain-only) are ever shown."
          className="mt-4"
        >
          <div className="grid grid-cols-1 gap-x-6 gap-y-1 font-mono text-[11px] text-ink4 sm:grid-cols-2">
            <span>
              accounts: <span className="text-ink3">{accounts.length}</span>
            </span>
            <span>
              calendars:{" "}
              <span className="text-ink3">
                {accounts.reduce((n, a) => n + calendarsFor(a.id).length, 0)}
              </span>
            </span>
            <span>
              selected:{" "}
              <span className="text-ink3">
                {accounts.reduce(
                  (n, a) => n + calendarsFor(a.id).filter((c) => c.selected).length,
                  0,
                )}
              </span>
            </span>
            <span>
              last_sync: <span className="text-ink3">{overview.last_sync ?? "never"}</span>
            </span>
            {accounts.map((a) => (
              <span key={a.id}>
                {a.email}: <span className="text-ink3">{a.state}</span>
              </span>
            ))}
          </div>
        </DevPanel>
      )}

      <ConfirmDialog
        open={confirmEmail != null}
        title={`Disconnect this ${meta.label} account?`}
        danger
        confirmLabel="Disconnect"
        onConfirm={() => {
          const email = confirmEmail;
          setConfirmEmail(null);
          if (email) void disconnect(email);
        }}
        onClose={() => setConfirmEmail(null)}
      >
        This signs out of that account and clears its mirrored events. Your saved credentials are
        kept, so you can reconnect without re-entering them.
      </ConfirmDialog>
    </div>
  );
}

/** One connected account: its email + reachability dot, a Disconnect, and the per-account calendar
 *  picker (which calendars to sync). */
function AccountBlock({
  account,
  calendars,
  busy,
  onToggle,
  onDisconnect,
}: {
  account: CalendarAccount;
  calendars: Calendar[];
  busy: boolean;
  onToggle: (cal: Calendar, on: boolean) => void;
  onDisconnect: () => void;
}) {
  const unreachable = account.state !== "ok";
  return (
    <div>
      <div className="flex items-center justify-between gap-2">
        <div className="flex min-w-0 items-center gap-2">
          <span className="truncate text-sm text-ink">{account.email ?? account.label}</span>
          {unreachable ? (
            <span className="shrink-0 text-[10px] uppercase tracking-wide text-st-due">
              unreachable
            </span>
          ) : (
            <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-[var(--st-quick)]" />
          )}
        </div>
        <Button
          variant="tertiary"
          onClick={onDisconnect}
          disabled={busy}
          className="shrink-0 px-2 py-1 text-xs hover:text-st-due"
        >
          Disconnect
        </Button>
      </div>
      {calendars.length > 0 ? (
        <ul className="mt-1.5 max-h-44 overflow-y-auto">
          {calendars.map((c) => (
            <li key={c.id} className="flex items-center gap-2 py-1 text-sm text-ink">
              <input
                type="checkbox"
                checked={c.selected}
                disabled={busy}
                onChange={(e) => onToggle(c, e.target.checked)}
                className="accent-[var(--accent)]"
              />
              <span className="truncate">{c.name}</span>
              {c.is_primary && <span className="font-mono text-[10px] text-ink4">primary</span>}
            </li>
          ))}
        </ul>
      ) : (
        <p className="mt-1 text-xs text-ink4">No calendars found on this account.</p>
      )}
    </div>
  );
}

/** Recognise Google's "the Calendar API isn't enabled for your Cloud project" 403 and pull the
 *  project-specific enable URL out of the message. Returns null for any other error (shown verbatim). */
function calendarApiDisabled(error: string | null): { enableUrl: string } | null {
  if (!error) return null;
  if (!/has not been used in project|accessNotConfigured/i.test(error)) return null;
  const url = error.match(/https?:\/\/[^\s"']+/)?.[0];
  return {
    enableUrl: url ?? "https://console.cloud.google.com/apis/library/calendar-json.googleapis.com",
  };
}

function formatWhen(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
}
