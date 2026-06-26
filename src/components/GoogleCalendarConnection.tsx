// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useState } from "react";
import {
  calendarStatus,
  connectGoogle,
  disconnectGoogle,
  listGoogleCalendars,
  setGoogleCalendarIds,
  syncCalendar,
} from "../lib/ipc";
import type { CalendarStatus, GoogleCalendar } from "../lib/types";
import { useDevMode } from "../lib/capabilities";
import { Button, ConfirmDialog, Skeleton } from "./ui";
import { DevPanel } from "./dev/DevPanel";
import { GoogleCredentialBlock } from "./GoogleCredentialBlock";

/**
 * **Google Calendar** (read-only OAuth) — the per-service connection under the Connectors tab's
 * Calendar section. Moved verbatim from the old standalone Calendar settings tab; the backend
 * (`connect_google` / `sync_calendar` / the `calendar_events` mirror) is unchanged.
 *
 * It reuses the shared, provider-level {@link GoogleCredentialBlock} (the one BYO Google client)
 * for sign-in setup, then offers Connect → browser, a calendar picker, Sync, and Disconnect. Once
 * the user signs in, PM mirrors the selected calendars and uses them for the agenda, schedule
 * questions in chat, and the focus view's "Due soon" status.
 */
export function GoogleCalendarConnection() {
  const { devMode } = useDevMode();
  const [status, setStatus] = useState<CalendarStatus | null>(null);
  const [calendars, setCalendars] = useState<GoogleCalendar[] | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirmDisconnect, setConfirmDisconnect] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setStatus(await calendarStatus());
    } catch (e) {
      setError(String(e));
    }
  }, []);

  const loadCalendars = useCallback(async () => {
    try {
      setCalendars(await listGoogleCalendars());
    } catch (e) {
      setError(String(e));
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  useEffect(() => {
    if (status?.oauth_connected) void loadCalendars();
  }, [status?.oauth_connected, loadCalendars]);

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

  const connect = () =>
    run("connect", async () => {
      await connectGoogle();
      await refresh();
      await loadCalendars();
      const n = await syncCalendar().catch(() => 0);
      setNote(`Connected. Synced ${n} event${n === 1 ? "" : "s"}.`);
      await refresh();
    });

  const toggleCalendar = (id: string, on: boolean) =>
    run("select", async () => {
      const prev = calendars;
      const next = (calendars ?? []).map((c) => (c.id === id ? { ...c, selected: on } : c));
      setCalendars(next);
      try {
        await setGoogleCalendarIds(next.filter((c) => c.selected).map((c) => c.id));
      } catch (e) {
        setCalendars(prev); // roll back the optimistic toggle the backend rejected
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

  const disconnect = () =>
    run("disconnect", async () => {
      await disconnectGoogle();
      setCalendars(null);
      await refresh();
    });

  const configured = status?.oauth_client_configured ?? false;
  const connected = status?.oauth_connected ?? false;

  return (
    <div data-help="settings-calendar">
      <span className="text-sm font-medium text-ink">Google Calendar</span>
      <p className="mt-1 text-xs text-ink4">
        Read-only sign-in with your own Google client. Powers your agenda, schedule questions in
        chat, and the “Due soon” status when an event names a project.
      </p>

      {!connected && (
        <div className="mt-2">
          <GoogleCredentialBlock configured={configured} onChange={refresh} />
        </div>
      )}

      {configured && !connected && (
        <div className="mt-2">
          <Button
            variant="primary"
            onClick={connect}
            disabled={busy != null}
            className="disabled:opacity-40"
          >
            {busy === "connect" ? "Waiting for Google…" : "Connect Google Calendar"}
          </Button>
        </div>
      )}

      {connected && (
        <div className="mt-2 space-y-2">
          <div className="flex items-center justify-between">
            <span className="inline-flex items-center gap-1.5 text-xs text-st-quick">
              <span className="h-1.5 w-1.5 rounded-full bg-[var(--st-quick)]" /> Connected
            </span>
            <Button
              variant="tertiary"
              onClick={() => setConfirmDisconnect(true)}
              disabled={busy != null}
              className="px-2 py-1 text-xs"
            >
              Disconnect
            </Button>
          </div>
          {calendars == null ? (
            <div className="flex flex-col gap-1.5 py-1">
              {Array.from({ length: 3 }).map((_, i) => (
                <Skeleton key={i} className="h-7 w-full" />
              ))}
            </div>
          ) : (
            <ul className="max-h-40 overflow-y-auto rounded-[var(--radius)] border border-border">
              {calendars.map((c) => (
                <li key={c.id} className="flex items-center gap-2 px-3 py-1.5 text-sm text-ink">
                  <input
                    type="checkbox"
                    checked={c.selected}
                    disabled={busy != null}
                    onChange={(e) => toggleCalendar(c.id, e.target.checked)}
                    className="accent-[var(--accent)]"
                  />
                  <span className="truncate">{c.summary}</span>
                  {c.primary && <span className="font-mono text-[10px] text-ink4">primary</span>}
                </li>
              ))}
            </ul>
          )}
          <div className="flex items-center justify-between">
            <p className="text-xs text-ink4">
              {status?.last_sync
                ? `Last synced ${formatWhen(status.last_sync)} · ${status.window_days} days ahead`
                : `Not synced yet · ${status?.window_days ?? 21} days ahead`}
            </p>
            <Button
              onClick={sync}
              disabled={busy != null}
              className="px-2 py-1 text-xs disabled:opacity-40"
            >
              {busy === "sync" || busy === "select" ? "Syncing…" : "Sync now"}
            </Button>
          </div>
        </div>
      )}

      {note && <p className="mt-2 text-xs text-st-quick">{note}</p>}
      {error && (
        <p
          className="mt-2 rounded-[var(--radius)] px-3 py-2 text-xs text-st-due"
          style={{ background: "color-mix(in oklab, var(--st-due) 15%, transparent)" }}
        >
          {error}
        </p>
      )}

      {devMode && status && (
        <DevPanel
          title="Calendar sync state"
          helpId="dev-calendar"
          subtitle="Connection + last-sync diagnostics. No tokens or feed URLs (keychain-only) are ever shown."
          className="mt-4"
        >
          <div className="grid grid-cols-1 gap-x-6 gap-y-1 font-mono text-[11px] text-ink4 sm:grid-cols-2">
            <span>
              ics_feeds: <span className="text-ink3">{status.ics_feeds}</span>
            </span>
            <span>
              calendars_selected: <span className="text-ink3">{status.calendars_selected}</span>
            </span>
            <span>
              oauth_client_configured:{" "}
              <span className="text-ink3">{status.oauth_client_configured ? "yes" : "no"}</span>
            </span>
            <span>
              oauth_connected:{" "}
              <span className="text-ink3">{status.oauth_connected ? "yes" : "no"}</span>
            </span>
            <span>
              window_days: <span className="text-ink3">{status.window_days}</span>
            </span>
            <span>
              last_sync: <span className="text-ink3">{status.last_sync ?? "never"}</span>
            </span>
          </div>
        </DevPanel>
      )}

      <ConfirmDialog
        open={confirmDisconnect}
        title="Disconnect Google Calendar?"
        danger
        confirmLabel="Disconnect"
        onConfirm={() => {
          setConfirmDisconnect(false);
          void disconnect();
        }}
        onClose={() => setConfirmDisconnect(false)}
      >
        This signs out of Google and clears the mirrored events. Your saved credentials are kept, so
        you can reconnect without re-entering them.
      </ConfirmDialog>
    </div>
  );
}

function formatWhen(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
}
