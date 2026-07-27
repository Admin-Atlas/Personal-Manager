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
  setCalendarQuiet,
  setCalendarKind,
  syncCalendar,
} from "../lib/ipc";
import type { Calendar, CalendarAccount, CalendarOverview, EventKind } from "../lib/types";
import { useDevMode } from "../lib/capabilities";
import { formatWhen } from "../lib/format";
import { useBusyRun } from "../lib/useBusyRun";
import { Button, ConfirmDialog, Select, Skeleton } from "./ui";
import { DevPanel } from "./dev/DevPanel";
import { GoogleOwnProjectConnect } from "./GoogleOwnProjectConnect";

/** Microsoft's app-access management page. Microsoft has no programmatic token revocation (unlike
 *  Google's RFC-7009 revoke), so disconnecting can only forget the local token — fully removing PM's
 *  access is done by the user here (L-3). */
const MICROSOFT_APPS_URL = "https://account.live.com/consent/Manage";

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
  const { busy, error, setError, run: runBusy } = useBusyRun();
  const [note, setNote] = useState<string | null>(null);
  const [confirmEmail, setConfirmEmail] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    try {
      setOverview(await calendarOverview());
    } catch (e) {
      setError(String(e));
    }
    // setError is a stable useState setter (via useBusyRun) — listed to satisfy exhaustive-deps.
  }, [setError]);

  // Refetch on mount, and whenever the parent group reports the shared client changed.
  useEffect(() => {
    void refresh();
  }, [refresh, refreshSignal]);

  // Each action starts with a clean note line (the shared latch already clears the error).
  function run(label: string, fn: () => Promise<void>) {
    setNote(null);
    return runBusy(label, fn);
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

  // Mark a calendar quiet (or not). Unlike the sync tick this needs no re-sync — the events stay in
  // the mirror; only the assistant paths (briefing/flags/chat/focus/milestones) filter them out.
  const toggleQuiet = (cal: Calendar, on: boolean) =>
    run("quiet", async () => {
      setOverview((o) =>
        o
          ? {
              ...o,
              calendars: o.calendars.map((c) => (c.id === cal.id ? { ...c, quiet: on } : c)),
            }
          : o,
      );
      try {
        await setCalendarQuiet(cal.id, on);
      } catch (e) {
        await refresh();
        throw e;
      }
    });

  // Type a calendar work/personal. Like Quiet this is PM's own annotation rather than upstream data,
  // so no re-sync is needed — and it survives one, because the upsert only refreshes provider fields.
  const setKind = (cal: Calendar, kind: EventKind | null) =>
    run("kind", async () => {
      setOverview((o) =>
        o
          ? {
              ...o,
              calendars: o.calendars.map((c) => (c.id === cal.id ? { ...c, kind } : c)),
            }
          : o,
      );
      try {
        await setCalendarKind(cal.id, kind);
      } catch (e) {
        await refresh();
        throw e;
      }
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
                    onToggleQuiet={toggleQuiet}
                    onSetKind={setKind}
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
      {provider === "microsoft" && (
        <p className="mt-2 text-xs text-ink4">
          Disconnecting forgets PM&rsquo;s access on this device. Microsoft can&rsquo;t revoke an
          app&rsquo;s access from within the app, so to fully remove it, manage app access at{" "}
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
          <div className="grid grid-cols-1 gap-x-6 gap-y-1 font-mono text-[0.6875rem] text-ink4 sm:grid-cols-2">
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
  onToggleQuiet,
  onSetKind,
  onDisconnect,
}: {
  account: CalendarAccount;
  calendars: Calendar[];
  busy: boolean;
  onToggle: (cal: Calendar, on: boolean) => void;
  onToggleQuiet: (cal: Calendar, on: boolean) => void;
  onSetKind: (cal: Calendar, kind: EventKind | null) => void;
  onDisconnect: () => void;
}) {
  const unreachable = account.state !== "ok";
  return (
    <div>
      <div className="flex items-center justify-between gap-2">
        <div className="flex min-w-0 items-center gap-2">
          <span className="truncate text-sm text-ink">{account.email ?? account.label}</span>
          {unreachable ? (
            <span className="shrink-0 text-[0.625rem] uppercase tracking-wide text-st-due">
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
        <>
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
                {c.is_primary && (
                  <span className="font-mono text-[0.625rem] text-ink4">primary</span>
                )}
                {c.selected && (
                  <span className="ml-auto flex shrink-0 items-center gap-2">
                    <Select
                      compact
                      value={c.kind ?? ""}
                      disabled={busy}
                      aria-label={`Is ${c.name} a work or personal calendar?`}
                      title="Whether this calendar's events count as work or personal."
                      onChange={(e) => onSetKind(c, (e.target.value || null) as EventKind | null)}
                      className="text-[0.625rem]"
                    >
                      <option value="">Untyped</option>
                      <option value="work">Work</option>
                      <option value="personal">Personal</option>
                    </Select>
                    <label
                      className="flex cursor-pointer items-center gap-1 text-[0.625rem] text-ink4"
                      title="Keep this calendar on the Calendar tab, but leave it out of reminders, the daily briefing, and chat."
                    >
                      <input
                        type="checkbox"
                        checked={c.quiet}
                        disabled={busy}
                        onChange={(e) => onToggleQuiet(c, e.target.checked)}
                        className="accent-[var(--accent)]"
                      />
                      Quiet
                    </label>
                  </span>
                )}
              </li>
            ))}
          </ul>
          {calendars.some((c) => c.selected) && (
            <p className="mt-1 text-[0.625rem] text-ink4">
              &ldquo;Quiet&rdquo; keeps a calendar visible on the Calendar tab but out of reminders,
              the daily briefing, and chat.
            </p>
          )}
        </>
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
