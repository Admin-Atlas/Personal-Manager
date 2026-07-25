// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useState } from "react";
import {
  addIcsFeed,
  calendarOverview,
  listIcsFeeds,
  removeIcsFeed,
  syncCalendar,
} from "../lib/ipc";
import type { CalendarOverview, IcsFeedInfo } from "../lib/types";
import { formatWhen } from "../lib/format";
import { useBusyRun } from "../lib/useBusyRun";
import { Button, ConfirmDialog, Input } from "./ui";

/**
 * The **zero-auth calendar subscription**: paste a calendar's private "secret address in iCal format".
 * No sign-in, no Cloud project — works even on accounts under Advanced Protection.
 *
 * **Provider-parameterised.** With `provider` set (the Apple group) it manages just that provider's
 * subscriptions and tags new ones accordingly; with no `provider` (the general "Calendar
 * subscriptions" section) it manages every feed NOT owned by a dedicated provider group (i.e. not
 * Apple) and tags new ones `other`. The feed URLs are secret bearer links and never leave Rust (the
 * keychain holds them); only `{id,label,provider}` reach the UI.
 */
export function IcsFeedSubscription({ provider }: { provider?: "apple" } = {}) {
  const scoped = provider != null;
  const [feeds, setFeeds] = useState<IcsFeedInfo[]>([]);
  const [overview, setOverview] = useState<CalendarOverview | null>(null);
  const [feedLabel, setFeedLabel] = useState("");
  const [feedUrl, setFeedUrl] = useState("");
  const [showGuide, setShowGuide] = useState(false);
  const { busy, error, setError, run: runBusy } = useBusyRun();
  const [note, setNote] = useState<string | null>(null);
  const [confirm, setConfirm] = useState<{ id: string; label: string } | null>(null);

  const refresh = useCallback(async () => {
    try {
      const [f, o] = await Promise.all([listIcsFeeds(), calendarOverview()]);
      setFeeds(f);
      setOverview(o);
    } catch (e) {
      setError(String(e));
    }
    // setError is a stable useState setter (via useBusyRun) — listed to satisfy exhaustive-deps.
  }, [setError]);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  // Each action starts with a clean note line (the shared latch already clears the error).
  function run(label: string, fn: () => Promise<void>) {
    setNote(null);
    return runBusy(label, fn);
  }

  const addFeed = () =>
    run("add-feed", async () => {
      await addIcsFeed(feedLabel.trim(), feedUrl.trim(), provider ?? "other");
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

  // Scoped (Apple) shows just that provider's feeds; the general section shows everything NOT owned by
  // a dedicated provider group (Apple has its own), so each feed lives in exactly one place.
  const shown = scoped
    ? feeds.filter((f) => f.provider === provider)
    : feeds.filter((f) => f.provider !== "apple");

  return (
    <div data-help={scoped ? `connectors-ics-${provider}` : "connectors-ics"}>
      <div className="flex items-center gap-2">
        <span className="text-sm font-medium text-ink">
          {scoped ? "Apple Calendar" : "Calendar subscription (iCal)"}
        </span>
        <span
          className="rounded-[var(--radius-sm)] px-1.5 py-0.5 font-mono text-[0.625rem] uppercase tracking-wide text-st-quick"
          style={{ background: "color-mix(in oklab, var(--st-quick) 18%, transparent)" }}
        >
          no sign-in
        </span>
      </div>
      <p className="mt-1 text-xs text-ink4">
        {scoped ? (
          <>
            iCloud has no desktop sign-in, so add an Apple calendar by its public iCal/ICS link — no
            account needed. Read-only; powers your agenda, schedule questions in chat, and the “Due
            soon” status when an event names a project.
          </>
        ) : (
          <>
            Paste any calendar’s private iCal/ICS link — from{" "}
            <span className="text-ink2">Google</span>, <span className="text-ink2">Outlook</span>,
            or any provider. No OAuth — works even with Advanced Protection. Read-only; powers your
            agenda, schedule questions in chat, and the “Due soon” status when an event names a
            project.
          </>
        )}
      </p>
      <button
        onClick={() => setShowGuide((s) => !s)}
        className="mt-1 text-xs text-accent-text hover:brightness-110"
      >
        {showGuide ? "Hide" : "Where do I find this? →"}
      </button>
      {showGuide && <FeedGuide only={provider} />}

      {shown.length > 0 && (
        <ul className="mt-2 divide-y divide-rule rounded-[var(--radius)] border border-border">
          {shown.map((f) => (
            <li
              key={f.id}
              className="flex items-center justify-between px-3 py-1.5 text-sm text-ink"
            >
              <span className="truncate">{f.label}</span>
              <Button
                variant="tertiary"
                onClick={() => setConfirm({ id: f.id, label: f.label })}
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
          placeholder="Paste the calendar’s iCal/ICS URL (https://…)"
        />
        <Button
          onClick={addFeed}
          disabled={busy != null || !feedUrl.trim()}
          className="disabled:opacity-40"
        >
          {busy === "add-feed" ? "Adding…" : "Add feed"}
        </Button>
      </div>

      {shown.length > 0 && (
        <div className="mt-3 flex items-center justify-between">
          <p className="text-xs text-ink4">
            {overview?.last_sync
              ? `Last synced ${formatWhen(overview.last_sync)} · ${overview.window_days} days ahead`
              : `Not synced yet · ${overview?.window_days ?? 21} days ahead`}
          </p>
          <Button
            onClick={sync}
            disabled={busy != null}
            className="px-2 py-1 text-xs disabled:opacity-40"
          >
            {busy === "sync" ? "Syncing…" : "Sync now"}
          </Button>
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

      <ConfirmDialog
        open={confirm != null}
        title="Remove this calendar feed?"
        danger
        confirmLabel="Remove"
        onConfirm={() => {
          const c = confirm;
          setConfirm(null);
          if (c) void removeFeed(c.id);
        }}
        onClose={() => setConfirm(null)}
      >
        This removes “{confirm?.label}” and its mirrored events. You can re-add the feed URL
        anytime.
      </ConfirmDialog>
    </div>
  );
}

/** How to find a calendar's private iCal/ICS address. `only` narrows it to one provider (the Apple
 *  group shows just the iCloud steps); otherwise every common provider is listed. */
function FeedGuide({ only }: { only?: "apple" }) {
  const link = "text-accent-text underline hover:brightness-110";
  return (
    <div className="mt-2 space-y-2.5 rounded-[var(--radius)] bg-surface px-3 py-2 text-xs text-ink3">
      <p className="text-ink4">
        Find the calendar’s private link below, then paste it in. Keep it private — anyone with the
        link can read that calendar.
      </p>

      {!only && (
        <div>
          <div className="font-medium text-ink2">Google Calendar</div>
          <ol className="mt-1 space-y-0.5">
            <li>
              1. Open{" "}
              <a
                href="https://calendar.google.com/"
                target="_blank"
                rel="noreferrer"
                className={link}
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
              3. Under <span className="text-ink2">Integrate calendar</span>, copy the{" "}
              <span className="text-ink2">Secret address in iCal format</span>.
            </li>
          </ol>
        </div>
      )}

      {!only && (
        <div>
          <div className="font-medium text-ink2">Outlook (outlook.com / Microsoft 365)</div>
          <ol className="mt-1 space-y-0.5">
            <li>
              1. Open{" "}
              <a
                href="https://outlook.office.com/calendar/"
                target="_blank"
                rel="noreferrer"
                className={link}
              >
                Outlook Calendar
              </a>{" "}
              on the web → <span className="text-ink2">Settings</span> (gear) →{" "}
              <span className="text-ink2">Calendar → Shared calendars</span>.
            </li>
            <li>
              2. Under <span className="text-ink2">Publish a calendar</span>, pick the calendar and{" "}
              <span className="text-ink2">Can view all details</span>, then{" "}
              <span className="text-ink2">Publish</span>.
            </li>
            <li>
              3. Copy the <span className="text-ink2">ICS</span> link (not the HTML one). Or connect
              Outlook above for an automatic sign-in instead.
            </li>
          </ol>
        </div>
      )}

      <div>
        <div className="font-medium text-ink2">Apple iCloud</div>
        <ol className="mt-1 space-y-0.5">
          <li>
            1. Open{" "}
            <a
              href="https://www.icloud.com/calendar/"
              target="_blank"
              rel="noreferrer"
              className={link}
            >
              iCloud Calendar
            </a>{" "}
            on the web (or the Calendar app on Mac).
          </li>
          <li>
            2. Click the share icon next to the calendar → turn on{" "}
            <span className="text-ink2">Public Calendar</span>.
          </li>
          <li>
            3. <span className="text-ink2">Copy Link</span>. If it starts with{" "}
            <span className="font-mono text-[0.6875rem] text-ink2">webcal://</span>, change it to{" "}
            <span className="font-mono text-[0.6875rem] text-ink2">https://</span>.
          </li>
        </ol>
      </div>
    </div>
  );
}
