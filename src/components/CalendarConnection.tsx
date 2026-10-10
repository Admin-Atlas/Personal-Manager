// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useState } from "react";
import {
  calendarOverview,
  connectGoogleCalendarAccount,
  connectOutlookCalendar,
  disconnectGoogleCalendarAccount,
  disconnectOutlookCalendar,
  enableCalendarEditing,
  setCalendarEditingPaused,
  setCalendarSelected,
  setCalendarQuiet,
  setCalendarKind,
  syncCalendar,
} from "../lib/ipc";
import type {
  Calendar,
  CalendarAccount,
  CalendarOverview,
  EditingStatus,
  EventKind,
  GoogleDisconnect,
} from "../lib/types";
import { editingTurnedOff } from "../lib/calendarEditing";
import type { GoogleGrantOutcome } from "../lib/googleGrantNote";
import { useDevMode } from "../lib/capabilities";
import { formatWhen } from "../lib/format";
import { useBusyRun } from "../lib/useBusyRun";
import { Button, Callout, ConfirmDialog, Select, Skeleton } from "./ui";
import { DevPanel } from "./dev/DevPanel";
import { GoogleGrantNote } from "./GoogleGrantNote";
import { GoogleOwnProjectConnect } from "./GoogleOwnProjectConnect";

/** Microsoft's app-access management page. Microsoft has no programmatic token revocation (unlike
 *  Google's RFC-7009 revoke), so disconnecting can only forget the local token — fully removing PM's
 *  access is done by the user here (L-3). */
const MICROSOFT_APPS_URL = "https://account.live.com/consent/Manage";

/** The two OAuth calendar providers. Apple has no desktop OAuth, so it stays a (read-only)
 *  subscription (see {@link "./IcsFeedSubscription"}). */
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
    /** Google names the features that kept its access alive; Microsoft has nothing to report. */
    disconnect: (email: string) => Promise<GoogleDisconnect | void>;
    /** What the disconnect confirmation says happens to the sign-in. */
    disconnectNote: string;
  }
> = {
  google: {
    label: "Google Calendar",
    sign_in: "Google sign-in",
    blurb:
      "Sign in with your own Google client. Connect one or more Google accounts; PM powers your agenda, schedule questions in chat, and the “Due soon” status when an event names a project. PM only reads your calendars unless you turn on editing for an account.",
    connect: connectGoogleCalendarAccount,
    disconnect: disconnectGoogleCalendarAccount,
    disconnectNote:
      "If Google Drive or backups still use this account, PM keeps Google's permission for them (and the account's own sign-in client, if it has one) and tells you so; if you turned on editing, that permission still covers changing this calendar's events. Otherwise PM also asks Google to remove its access.",
  },
  microsoft: {
    label: "Outlook Calendar",
    sign_in: "Microsoft sign-in",
    blurb:
      "Read-only sign-in with your Microsoft 365 / Outlook account. Connect one or more accounts; PM powers your agenda, schedule questions in chat, and the “Due soon” status when an event names a project.",
    connect: connectOutlookCalendar,
    disconnect: disconnectOutlookCalendar,
    disconnectNote:
      "Your saved credentials are kept, so you can reconnect without re-entering them.",
  },
};

/**
 * **Calendar connection** (OAuth) — the per-provider account + calendar manager under the
 * Connectors tab's Google / Microsoft groups. Google Calendar and Outlook are near-identical (the only
 * differences are the connect/disconnect commands, a few labels, and Google's per-account editing
 * switch), so one provider-parameterised component serves both rather than two duplicated files.
 *
 * The shared, provider-level BYO OAuth client is set up once at the group level (see
 * {@link "./ConnectorsSettings"}). Once it's configured, this offers Connect → browser, a per-account
 * calendar picker, Sync, and Disconnect. **Multi-account:** connect several accounts of the same
 * provider; each is independent. `refreshSignal` is bumped by the parent group when the shared client
 * is saved/cleared, so this refetches `calendar_overview`.
 *
 * Google only: `grantOutcome` / `onGrantOutcome` lift the "PM kept its access because…" note to the
 * Google group, which Calendar and Drive share — a later action in EITHER section must replace it, or
 * one section keeps a note the other's disconnect has made untrue.
 */
export function CalendarConnection({
  provider,
  refreshSignal = 0,
  grantOutcome = null,
  onGrantOutcome,
}: {
  provider: Provider;
  refreshSignal?: number;
  grantOutcome?: GoogleGrantOutcome | null;
  onGrantOutcome?: (outcome: GoogleGrantOutcome | null) => void;
}) {
  const meta = PROVIDER_META[provider];
  const { devMode } = useDevMode();
  const [overview, setOverview] = useState<CalendarOverview | null>(null);
  const { busy, error, setError, run: runBusy } = useBusyRun();
  const [note, setNote] = useState<string | null>(null);
  const [confirmEmail, setConfirmEmail] = useState<string | null>(null);

  // Returns what it read, so an action can compare it with what was on screen before.
  const refresh = useCallback(async (): Promise<CalendarOverview | null> => {
    try {
      const fresh = await calendarOverview();
      setOverview(fresh);
      return fresh;
    } catch (e) {
      setError(String(e));
      return null;
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
    onGrantOutcome?.(null);
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
    // The own-project path calls this directly, outside `run`, so it clears the grant note itself.
    onGrantOutcome?.(null);
    const editingBefore = overview?.editing ?? {};
    const fresh = await refresh();
    const n = await syncCalendar().catch(() => 0);
    // Connecting again asks Google for reading only, which turns editing off on that account. Say so,
    // or editing would just quietly be gone.
    const turnedOff = editingTurnedOff(editingBefore, fresh?.editing ?? {})
      .map((id) => fresh?.accounts.find((a) => a.id === id)?.email)
      .filter((e): e is string => e != null);
    setNote(
      `Connected. Synced ${n} event${n === 1 ? "" : "s"}.` +
        (turnedOff.length > 0
          ? ` Editing is now off for ${turnedOff.join(", ")}, because connecting again only asks Google for reading. Turn it back on under the account.`
          : ""),
    );
    await refresh();
  };

  const connect = () =>
    run("connect", async () => {
      await meta.connect();
      await afterConnect();
    });

  const disconnect = (email: string) =>
    run("disconnect", async () => {
      const out = await meta.disconnect(email);
      if (out) {
        // The backend knows whether the kept grant still covers changing events: editing turned off
        // in PM (or by a read-only reconnect) leaves Google's grant as it was.
        onGrantOutcome?.({
          service: "calendar",
          email,
          keptFor: out.kept_for,
          calendarWrite: out.calendar_write,
        });
      }
      await refresh();
    });

  // Google asks for both permissions; leaving "change events" unticked keeps the account read-only.
  // The busy label names the account, so only its own button says "Waiting for Google…".
  const enableEditing = (email: string) =>
    run(`editing:${email}`, async () => {
      const status = await enableCalendarEditing(email);
      setNote(
        status === "on"
          ? `Editing is on for ${email}.`
          : `Google didn't give PM permission to change events (that box was left unticked), so ${email} stays read-only.`,
      );
      await refresh();
    });

  // Switching off is PM's alone: Google keeps the permission, so switching back needs no sign-in.
  const pauseEditing = (email: string, paused: boolean) =>
    run(`pause:${email}`, async () => {
      await setCalendarEditingPaused(email, paused);
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
      {/* Accounts left from a sign-in that has since been cleared (an older PM cleared the Microsoft
          client without removing Outlook calendars): they can't sync, and with the list hidden behind
          `configured` there was no way to remove them. Disconnect needs no client. */}
      {!configured && accounts.length > 0 && (
        <>
          <p className="mt-2 text-xs text-ink3">
            These accounts were connected with a sign-in that has since been cleared, so they
            can&rsquo;t sync. Disconnect them here, or set the sign-in up again above.
          </p>
          <ul className="mt-2 divide-y divide-rule rounded-[var(--radius)] border border-border">
            {accounts.map((a) => (
              <li key={a.id} className="flex items-center justify-between gap-2 px-3 py-2">
                <span className="truncate text-xs text-ink2">{a.email ?? a.label}</span>
                <Button
                  size="sm"
                  variant="secondary"
                  onClick={() => setConfirmEmail(a.email)}
                  disabled={busy != null || a.email == null}
                >
                  Disconnect
                </Button>
              </li>
            ))}
          </ul>
        </>
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
                  {provider === "google" && a.email != null && (
                    <EditingControl
                      email={a.email}
                      status={overview.editing[a.id] ?? "off"}
                      busy={busy}
                      onEnable={enableEditing}
                      onPause={pauseEditing}
                    />
                  )}
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
              <Button size="sm" onClick={sync} disabled={busy != null}>
                {busy === "sync" || busy === "select" ? "Syncing…" : "Sync now"}
              </Button>
            )}
          </div>

          <div className="mt-2">
            <Button
              variant={accounts.length === 0 ? "primary" : "secondary"}
              onClick={connect}
              disabled={busy != null}
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
      {provider === "google" && (
        <GoogleGrantNote outcome={grantOutcome?.service === "calendar" ? grantOutcome : null} />
      )}
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
          <Callout className="mt-2">
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
          </Callout>
        ) : (
          <Callout as="p" className="mt-2">
            {error}
          </Callout>
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
        This signs out of that account and clears its mirrored events. {meta.disconnectNote}
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
  // 'error' means the sync RAN but couldn't see the whole calendar (a page-capped fetch, or an ICS
  // body cut mid-event). Nothing was removed from the mirror and the account is perfectly reachable,
  // so the shared "unreachable" wording would be an outright lie — mirrors CloudDriveConnection.
  const partial = account.state === "error";
  const unreachable = account.state !== "ok";
  return (
    <div>
      <div className="flex items-center justify-between gap-2">
        <div className="flex min-w-0 items-center gap-2">
          <span className="truncate text-sm text-ink">{account.email ?? account.label}</span>
          {unreachable ? (
            <span className="shrink-0 text-[0.625rem] uppercase tracking-wide text-st-due">
              {partial ? "sync didn’t finish" : "unreachable"}
            </span>
          ) : (
            <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-[var(--st-quick)]" />
          )}
        </div>
        <Button
          variant="tertiary"
          size="sm"
          onClick={onDisconnect}
          disabled={busy}
          className="shrink-0 hover:text-st-due"
        >
          Disconnect
        </Button>
      </div>
      {partial && (
        <p className="mt-0.5 text-xs text-ink4">
          The last sync didn&rsquo;t reach the end of this calendar. The events already mirrored are
          kept — nothing was removed — and the next sync picks up the rest.
        </p>
      )}
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

/** What each editing status says, and the one action it offers. */
const EDITING_COPY: Record<
  EditingStatus,
  { text: string; action: string; enable: boolean; paused?: boolean }
> = {
  off: {
    text: "Read-only. Turn on editing to create, change and delete this account’s events from PM. Google asks you to allow two things: leave both ticked.",
    action: "Turn on editing…",
    enable: true,
  },
  on: {
    text: "Editing on: PM can create, change and delete events on this account’s calendars.",
    action: "Turn off editing",
    enable: false,
    paused: true,
  },
  paused: {
    text: "Editing is off in PM. Google still allows it, so turning it back on doesn’t ask you to sign in.",
    action: "Turn editing back on",
    enable: false,
    paused: false,
  },
  needs_consent: {
    text: "Editing is turned on, but Google no longer gives PM permission to change this account’s events.",
    action: "Ask Google again…",
    enable: true,
  },
};

/** One Google account's editing switch (#884). Off until the user turns it on through Google's
 *  consent; once on, switching it off and back is PM's alone, with no sign-in. */
function EditingControl({
  email,
  status,
  busy,
  onEnable,
  onPause,
}: {
  email: string;
  status: EditingStatus;
  busy: string | null;
  onEnable: (email: string) => void;
  onPause: (email: string, paused: boolean) => void;
}) {
  const copy = EDITING_COPY[status];
  return (
    <div
      className="mt-1.5 flex items-start justify-between gap-2"
      data-help="settings-calendar-editing"
    >
      <p className={`min-w-0 text-xs ${status === "on" ? "text-ink3" : "text-ink4"}`}>
        {copy.text}
      </p>
      <Button
        size="sm"
        variant={copy.enable ? "secondary" : "tertiary"}
        disabled={busy != null}
        className="shrink-0"
        onClick={() => (copy.enable ? onEnable(email) : onPause(email, copy.paused ?? false))}
      >
        {busy === `editing:${email}` ? "Waiting for Google…" : copy.action}
      </Button>
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
