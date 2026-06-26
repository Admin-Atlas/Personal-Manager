// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useState } from "react";
import {
  addIcsFeed,
  calendarStatus,
  clearGoogleClient,
  connectGoogle,
  disconnectGoogle,
  listGoogleCalendars,
  listIcsFeeds,
  removeIcsFeed,
  setGoogleCalendarIds,
  setGoogleClient,
  syncCalendar,
} from "../lib/ipc";
import type { CalendarStatus, GoogleCalendar, IcsFeedInfo } from "../lib/types";
import { useDevMode } from "../lib/capabilities";
import { Button, ConfirmDialog, Input, Skeleton } from "./ui";
import { DevPanel } from "./dev/DevPanel";

type Confirm = { kind: "disconnect" } | { kind: "remove-feed"; id: string; label: string };

/**
 * The read-only calendar connector (Step 6). Two paths:
 *  - **iCal feeds (default)** — paste a calendar's private "secret address in iCal
 *    format". No sign-in, no Google Cloud project, and it works even on accounts in
 *    Google's Advanced Protection Program (which blocks unverified OAuth apps).
 *  - **Google sign-in (advanced)** — full OAuth with your own client credentials.
 *
 * Either way PM mirrors events locally and uses them for the agenda, chat context,
 * and the focus view's "Due soon" status. Tokens/feed URLs live in the keychain.
 */
export function CalendarSettings() {
  const { devMode } = useDevMode();
  const [status, setStatus] = useState<CalendarStatus | null>(null);
  const [feeds, setFeeds] = useState<IcsFeedInfo[]>([]);
  const [feedLabel, setFeedLabel] = useState("");
  const [feedUrl, setFeedUrl] = useState("");
  const [showFeedGuide, setShowFeedGuide] = useState(false);
  const [showAdvanced, setShowAdvanced] = useState(false);

  // OAuth (advanced) state.
  const [clientId, setClientId] = useState("");
  const [clientSecret, setClientSecret] = useState("");
  const [calendars, setCalendars] = useState<GoogleCalendar[] | null>(null);

  const [busy, setBusy] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<Confirm | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [s, f] = await Promise.all([calendarStatus(), listIcsFeeds()]);
      setStatus(s);
      setFeeds(f);
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

  const active = (status?.ics_feeds ?? 0) > 0 || (status?.oauth_connected ?? false);

  // --- iCal feeds ---
  const addFeed = () =>
    run("add-feed", async () => {
      await addIcsFeed(feedLabel.trim(), feedUrl.trim());
      setFeedLabel("");
      setFeedUrl("");
      await refresh();
      setNote("Calendar feed added and synced.");
    });

  const removeFeed = (id: string) =>
    run("remove-feed", async () => {
      await removeIcsFeed(id);
      await refresh();
    });

  const sync = () =>
    run("sync", async () => {
      const n = await syncCalendar();
      setNote(`Synced ${n} event${n === 1 ? "" : "s"}.`);
      await refresh();
    });

  // --- Google OAuth (advanced) ---
  const saveCreds = () =>
    run("save", async () => {
      await setGoogleClient(clientId.trim(), clientSecret.trim());
      setClientId("");
      setClientSecret("");
      await refresh();
    });

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

  const disconnect = () =>
    run("disconnect", async () => {
      await disconnectGoogle();
      setCalendars(null);
      await refresh();
    });

  const forgetCreds = () =>
    run("forget", async () => {
      await clearGoogleClient();
      setCalendars(null);
      await refresh();
    });

  return (
    <div className="mt-5 border-t border-border pt-4" data-help="settings-calendar">
      <label className="block text-sm font-medium text-ink2">Calendar</label>
      <p className="mt-1 text-xs text-ink4">
        Read-only. Powers your agenda, schedule questions in chat, and the “Due soon” status when an
        event names a project. Everything stays in your keychain / local store.
      </p>

      {/* iCal feeds — the simple default path. */}
      <div className="mt-3 rounded-[var(--radius)] border border-border p-3">
        <div className="flex items-center gap-2">
          <span className="text-sm font-medium text-ink">Calendar feed (iCal)</span>
          <span
            className="rounded-[var(--radius-sm)] px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-wide text-st-quick"
            style={{ background: "color-mix(in oklab, var(--st-quick) 18%, transparent)" }}
          >
            recommended
          </span>
        </div>
        <p className="mt-1 text-xs text-ink4">
          Paste a calendar’s private “secret address in iCal format”. No sign-in — works even with
          Advanced Protection.
        </p>
        <button
          onClick={() => setShowFeedGuide((s) => !s)}
          className="mt-1 text-xs text-accent-text hover:brightness-110"
        >
          {showFeedGuide ? "Hide" : "Where do I find this? →"}
        </button>
        {showFeedGuide && <FeedGuide />}

        {feeds.length > 0 && (
          <ul className="mt-2 divide-y divide-rule rounded-[var(--radius)] border border-border">
            {feeds.map((f) => (
              <li
                key={f.id}
                className="flex items-center justify-between px-3 py-1.5 text-sm text-ink"
              >
                <span className="truncate">{f.label}</span>
                <Button
                  variant="tertiary"
                  onClick={() => setConfirm({ kind: "remove-feed", id: f.id, label: f.label })}
                  disabled={busy != null}
                  className="shrink-0 px-2 py-0.5 text-xs hover:text-st-due"
                >
                  Remove
                </Button>
              </li>
            ))}
          </ul>
        )}

        <div className="mt-2 space-y-2">
          <Input
            type="text"
            value={feedLabel}
            onChange={(e) => setFeedLabel(e.target.value)}
            placeholder="Label (optional, e.g. Work)"
          />
          <Input
            type="text"
            autoComplete="off"
            value={feedUrl}
            onChange={(e) => setFeedUrl(e.target.value)}
            placeholder="https://calendar.google.com/calendar/ical/…/basic.ics"
          />
          <Button
            onClick={addFeed}
            disabled={busy != null || !feedUrl.trim()}
            className="disabled:opacity-40"
          >
            {busy === "add-feed" ? "Adding…" : "Add feed"}
          </Button>
        </div>
      </div>

      {/* Shared sync + last-sync line. */}
      {active && (
        <div className="mt-3 flex items-center justify-between">
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
      )}

      {/* Google sign-in — advanced/optional path. */}
      <div className="mt-4">
        <button
          onClick={() => setShowAdvanced((s) => !s)}
          className="text-xs text-ink3 hover:text-ink"
        >
          {showAdvanced ? "▾" : "▸"} Advanced: connect with Google sign-in (OAuth)
        </button>
        {showAdvanced && (
          <div className="mt-2 rounded-[var(--radius)] border border-border p-3">
            <p className="text-xs text-ink4">
              Full OAuth with your own Google “Desktop app” credentials. Note: if your account uses
              Advanced Protection, Google blocks this — use a calendar feed above instead.
            </p>

            {!status?.oauth_client_configured ? (
              <div className="mt-2 space-y-2">
                <Input
                  type="text"
                  autoComplete="off"
                  value={clientId}
                  onChange={(e) => setClientId(e.target.value)}
                  placeholder="Client ID (…apps.googleusercontent.com)"
                />
                <Input
                  type="password"
                  autoComplete="off"
                  value={clientSecret}
                  onChange={(e) => setClientSecret(e.target.value)}
                  placeholder="Client secret"
                />
                <Button
                  onClick={saveCreds}
                  disabled={busy != null || !clientId.trim() || !clientSecret.trim()}
                  className="disabled:opacity-40"
                >
                  {busy === "save" ? "Saving…" : "Save credentials"}
                </Button>
              </div>
            ) : !status.oauth_connected ? (
              <div className="mt-2 flex flex-wrap items-center gap-2">
                <Button
                  variant="primary"
                  onClick={connect}
                  disabled={busy != null}
                  className="disabled:opacity-40"
                >
                  {busy === "connect" ? "Waiting for Google…" : "Connect Google Calendar"}
                </Button>
                <Button
                  variant="tertiary"
                  onClick={forgetCreds}
                  disabled={busy != null}
                  className="px-2 py-1.5 text-xs"
                >
                  Change credentials
                </Button>
              </div>
            ) : (
              <div className="mt-2 space-y-2">
                <div className="flex items-center justify-between">
                  <span className="inline-flex items-center gap-1.5 text-xs text-st-quick">
                    <span className="h-1.5 w-1.5 rounded-full bg-[var(--st-quick)]" /> Connected
                  </span>
                  <Button
                    variant="tertiary"
                    onClick={() => setConfirm({ kind: "disconnect" })}
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
                      <li
                        key={c.id}
                        className="flex items-center gap-2 px-3 py-1.5 text-sm text-ink"
                      >
                        <input
                          type="checkbox"
                          checked={c.selected}
                          disabled={busy != null}
                          onChange={(e) => toggleCalendar(c.id, e.target.checked)}
                          className="accent-[var(--accent)]"
                        />
                        <span className="truncate">{c.summary}</span>
                        {c.primary && (
                          <span className="font-mono text-[10px] text-ink4">primary</span>
                        )}
                      </li>
                    ))}
                  </ul>
                )}
              </div>
            )}
          </div>
        )}
      </div>

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
        open={confirm != null}
        title={
          confirm?.kind === "disconnect"
            ? "Disconnect Google Calendar?"
            : "Remove this calendar feed?"
        }
        danger
        confirmLabel={confirm?.kind === "disconnect" ? "Disconnect" : "Remove"}
        onConfirm={() => {
          const c = confirm;
          setConfirm(null);
          if (c?.kind === "disconnect") void disconnect();
          else if (c?.kind === "remove-feed") void removeFeed(c.id);
        }}
        onClose={() => setConfirm(null)}
      >
        {confirm?.kind === "disconnect" ? (
          "This signs out of Google and clears the mirrored events. Your saved credentials are kept, so you can reconnect without re-entering them."
        ) : (
          <>
            This removes “{confirm?.kind === "remove-feed" ? confirm.label : "this feed"}” and its
            mirrored events. You can re-add the feed URL anytime.
          </>
        )}
      </ConfirmDialog>
    </div>
  );
}

/** How to find a Google Calendar's private iCal address. */
function FeedGuide() {
  return (
    <ol className="mt-2 space-y-1 rounded-[var(--radius)] border border-border bg-surface px-3 py-2 text-xs text-ink3">
      <li>
        1. Open{" "}
        <a
          href="https://calendar.google.com/"
          target="_blank"
          rel="noreferrer"
          className="text-accent-text underline hover:brightness-110"
        >
          Google Calendar
        </a>{" "}
        on the web.
      </li>
      <li>
        2. Hover the calendar in the left list → ⋮ →{" "}
        <span className="text-ink2">Settings and sharing</span>.
      </li>
      <li>
        3. Scroll to <span className="text-ink2">Integrate calendar</span> → copy the{" "}
        <span className="text-ink2">Secret address in iCal format</span>.
      </li>
      <li>4. Paste it below. (Keep it private — anyone with the link can read that calendar.)</li>
    </ol>
  );
}

function formatWhen(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
}
