// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useState, type ReactNode } from "react";
import { useDepth } from "../theme";
import { driveStatus, oneDriveStatus } from "../lib/ipc";
import { Collapsible, SegmentedControl } from "./ui";
import { CalendarConnection } from "./CalendarConnection";
import { CloudDriveConnection } from "./CloudDriveConnection";
import { LocalFolderConnection } from "./LocalFolderConnection";
import { GoogleCredentialBlock } from "./GoogleCredentialBlock";
import { MicrosoftCredentialBlock } from "./MicrosoftCredentialBlock";
import { IcsFeedSubscription } from "./IcsFeedSubscription";

/**
 * The **Connectors** hub (Stage 3, §8.1) — where external accounts are connected so the Archivist
 * can index them, and where the read-only calendar lives.
 *
 * Grouped **by provider** (Google / Microsoft / Apple). A provider's BYO-OAuth client is
 * provider-level — one client powers all of that provider's services (Google: Calendar + Drive +
 * future Gmail; Microsoft: OneDrive + future Outlook) — so it's set up **once at the top of the
 * provider group** (see {@link GoogleProvider} / {@link MicrosoftProvider}), with the multi-account
 * guidance right beside it. Previously the shared client + that guidance were buried inside whichever
 * service you happened to open first (so e.g. a Calendar-first user never saw the Drive-only
 * "Add another account" flow); grouping by provider gives them one obvious home.
 *
 * Calendar subscriptions (zero-auth iCal) are provider-agnostic, so they get their own section at the
 * bottom. Every connection is **independently opt-in and removable** — set up a provider's client and
 * connect nothing, subscribe to a calendar URL only, connect Drive only; nothing cascades.
 */
export function ConnectorsSettings({
  indexingSpeed,
  onChangeIndexingSpeed,
}: {
  indexingSpeed: "fast" | "gentle";
  onChangeIndexingSpeed: (speed: "fast" | "gentle") => void;
}) {
  return (
    <div className="mt-5 border-t border-border pt-4" data-help="settings-connectors">
      <label className="block text-sm font-medium text-ink2">Connectors</label>
      <p className="mt-1 text-xs text-ink4">
        Connect external accounts so PM can find and use what’s in them, grouped by provider. Set up
        a provider’s sign-in once and it’s shared across that provider’s services. Every connection
        is independently opt-in and removable — nothing cascades. Credentials and tokens live only
        in your keychain.
      </p>

      <IndexingSpeedControl value={indexingSpeed} onChange={onChangeIndexingSpeed} />

      <GoogleProvider />
      <MicrosoftProvider />
      <AppleProvider />
      <ThisDevice />
      <CalendarSubscriptions />
    </div>
  );
}

/** The **This device** group — sources that live on this machine, no provider or sign-in. Currently
 *  just local folders (board card 6); iCloud/OneDrive local mirrors would slot in here too. Kept apart
 *  from the provider groups because it belongs to no cloud account and takes no OAuth client. */
function ThisDevice() {
  return (
    <ConnectorGroup
      title="This device"
      blurb="Index folders on this computer. Everything is index-only — a searchable pointer and summary; the files stay on disk. PM watches each folder and keeps it current as you work."
    >
      <LocalFolderConnection />
    </ConnectorGroup>
  );
}

/**
 * The **Google** provider group: the one BYO Google client (shared by Calendar, Drive, future Gmail)
 * set up once at the top, the "more than one account?" guidance beside it, then each Google service.
 *
 * The shared client is **provider-level** (`google::has_client()` — a single global flag), so this
 * group owns the `configured` flag (read from `drive_status`, which carries that flag) and renders
 * {@link GoogleCredentialBlock} once. Saving/clearing it bumps `signal`, which the child services
 * watch (via their `refreshSignal` prop) to refetch — so their connect buttons / account lists track
 * the new client state without each embedding its own copy of the setup block.
 */
function GoogleProvider() {
  const [configured, setConfigured] = useState(false);
  const [signal, setSignal] = useState(0);

  const refreshConfigured = useCallback(async () => {
    try {
      // drive_status carries the provider-level `google::has_client()` flag; reading it here avoids a
      // dedicated command while keeping the shared sign-in block's source of truth in one place.
      const s = await driveStatus();
      setConfigured(s.oauth_client_configured);
    } catch {
      // The child connectors surface their own errors; a failed flag read just leaves setup showing.
    }
  }, []);

  useEffect(() => {
    void refreshConfigured();
  }, [refreshConfigured]);

  const onClientChange = useCallback(async () => {
    await refreshConfigured();
    setSignal((n) => n + 1);
  }, [refreshConfigured]);

  return (
    <ConnectorGroup
      title="Google"
      blurb="One sign-in powers Calendar and Drive (Gmail later). Set it up once below; each calendar and account stays independently opt-in."
    >
      <GoogleCredentialBlock configured={configured} onChange={onClientChange} />
      {configured && <GoogleMultiAccountHelp />}
      <Divider />
      <CalendarConnection provider="google" refreshSignal={signal} />
      <Divider />
      <CloudDriveConnection provider="google" refreshSignal={signal} />
      <Divider />
      <ComingSoonRow name="Gmail" />
    </ConnectorGroup>
  );
}

/**
 * The **Microsoft** provider group — mirrors {@link GoogleProvider}. The shared Microsoft client is a
 * public client (just a client ID, no secret); reads the provider-level `configured` flag from
 * `onedrive_status` and lifts {@link MicrosoftCredentialBlock} above OneDrive.
 */
function MicrosoftProvider() {
  const [configured, setConfigured] = useState(false);
  const [signal, setSignal] = useState(0);

  const refreshConfigured = useCallback(async () => {
    try {
      const s = await oneDriveStatus();
      setConfigured(s.oauth_client_configured);
    } catch {
      // OneDrive surfaces its own errors; a failed flag read just leaves setup showing.
    }
  }, []);

  useEffect(() => {
    void refreshConfigured();
  }, [refreshConfigured]);

  const onClientChange = useCallback(async () => {
    await refreshConfigured();
    setSignal((n) => n + 1);
  }, [refreshConfigured]);

  return (
    <ConnectorGroup
      title="Microsoft"
      blurb="One sign-in powers OneDrive and Outlook Calendar (Mail later). It’s a public app registration — just a client ID, no secret."
    >
      <MicrosoftCredentialBlock configured={configured} onChange={onClientChange} />
      <Divider />
      <CloudDriveConnection provider="microsoft" refreshSignal={signal} />
      <Divider />
      <CalendarConnection provider="microsoft" refreshSignal={signal} />
      <Divider />
      <ComingSoonRow name="Outlook Mail" detail="Microsoft — coming later." />
    </ConnectorGroup>
  );
}

/** The **Apple** provider group — placeholders only for now. Apple/iCloud connectors land later;
 *  meanwhile an Apple (or any) calendar can be added below as a no-sign-in subscription. */
function AppleProvider() {
  return (
    <ConnectorGroup
      title="Apple"
      blurb="Apple has no desktop sign-in, so add an Apple/iCloud calendar by its public link — no account needed. iCloud Drive is coming later."
    >
      <IcsFeedSubscription provider="apple" />
      <Divider />
      <ComingSoonRow name="iCloud Drive" detail="Apple — coming later." />
    </ConnectorGroup>
  );
}

/** The provider-agnostic **calendar subscriptions** (iCal) section — no sign-in, no provider account,
 *  works even under Advanced Protection. Kept separate from the provider groups because it belongs to
 *  no one provider. */
function CalendarSubscriptions() {
  return (
    <ConnectorGroup
      title="Calendar subscriptions"
      blurb="Add any other calendar by its private iCal/ICS link — no sign-in, even under Advanced Protection. (Google and Outlook can sign in above; Apple has its own section.) Read-only: it powers your agenda, schedule questions in chat, and the “Due soon” status."
    >
      <IcsFeedSubscription />
    </ConnectorGroup>
  );
}

/**
 * The provider-level "more than one Google account?" guidance — the discoverability fix. It lives at
 * the top of the Google group (not buried inside Drive), so a user looking to connect a second Google
 * account finds it whatever service they came for. Reuses the same facts as the BYO-client setup
 * guide: reuse the one project, add each account as a Test user, then the account chooser does the
 * rest (PM now forces it — see `google::build_auth_url`).
 */
function GoogleMultiAccountHelp() {
  const link = "text-accent-text underline hover:brightness-110";
  return (
    <Collapsible
      className="mt-2"
      defaultOpen={false}
      title={<span className="text-xs text-ink3">Using more than one Google account?</span>}
    >
      <div
        className="mt-1.5 space-y-1.5 rounded-[var(--radius)] bg-surface px-3 py-2 text-xs text-ink3"
        data-help="connectors-google-multiaccount"
      >
        <p>
          You don’t need a new project or new credentials — every account reuses the one Client ID +
          secret you saved above.
        </p>
        <p>
          <span className="text-ink2">1. Authorise the account in Google Cloud.</span> If your OAuth
          app is still in <span className="text-ink2">Testing</span> mode, add each account’s email
          under{" "}
          <a
            href="https://console.cloud.google.com/auth/audience"
            target="_blank"
            rel="noreferrer"
            className={link}
          >
            Audience → Test users
          </a>{" "}
          (<span className="text-ink2">+ Add users</span>) in the Google Cloud Console.
        </p>
        <p className="text-ink4">
          Testing mode signs accounts out after <span className="text-ink2">7 days</span>. For a
          connection that lasts, <span className="text-ink2">publish to Production</span> on that
          same{" "}
          <a
            href="https://console.cloud.google.com/auth/audience"
            target="_blank"
            rel="noreferrer"
            className={link}
          >
            Audience
          </a>{" "}
          page (<span className="text-ink2">Publish app</span>) instead — no test-user list to keep
          and no 7-day expiry. The “unverified app” screen is expected for your own client; continue
          past it.
        </p>
        <p>
          <span className="text-ink2">2. Add it in PM.</span> Use{" "}
          <span className="text-ink2">Add another account</span> on Calendar or Drive below — Google
          shows its account chooser, so pick the <em>different</em> account. Each is independent.
        </p>
        <p className="text-ink4">
          Prefer to keep accounts fully separate? You can instead make a second Google Cloud project
          with its own credentials — but for most people reusing the one project is simpler.
        </p>
      </div>
    </Collapsible>
  );
}

/**
 * A connector group (Google / Microsoft / Apple / Calendar subscriptions) holding a provider's shared
 * sign-in and its services. These get large (multi-account Drive, calendar feeds), so each is
 * **collapsible**: open by default for Standard/Power density, **collapsed at Minimal** (so the tab
 * stays scannable), always toggleable. The disclosure state is per-group local UI.
 */
function ConnectorGroup({
  title,
  blurb,
  children,
}: {
  title: string;
  blurb?: string;
  children: ReactNode;
}) {
  const { minimal } = useDepth();
  return (
    <Collapsible
      className="mt-4 rounded-[var(--radius)] border border-border p-3"
      defaultOpen={!minimal}
      title={
        <span className="font-mono text-xs font-medium uppercase tracking-wide text-ink3">
          {title}
        </span>
      }
    >
      {blurb && <p className="mt-1 text-xs text-ink4">{blurb}</p>}
      <div className="mt-3">{children}</div>
    </Collapsible>
  );
}

/**
 * The **Indexing speed** control — the Fast/Gentle pacing toggle, surfaced here at the top of the
 * Connectors hub because that's where it bites most (a Drive sync is the big, long-running index).
 * Fast runs at full throughput; Gentle paces each file AND embeds in smaller batches so a low-end
 * machine stays responsive on both CPU and memory. The setting is re-read mid-run, so a switch
 * applies to the very next file — even partway through a sync.
 */
function IndexingSpeedControl({
  value,
  onChange,
}: {
  value: "fast" | "gentle";
  onChange: (speed: "fast" | "gentle") => void;
}) {
  // The Fast/Gentle detail is a wall of text most people set once and forget, so it's collapsed by
  // default — but **open for Power** density (who tune this kind of thing), collapsed at Minimal/
  // Standard. The control + one-line summary stay visible at every depth.
  const { showPower } = useDepth();
  return (
    <div
      className="mt-4 rounded-[var(--radius)] border border-border p-3"
      data-help="settings-indexing-speed"
    >
      <div className="flex items-start justify-between gap-3">
        <div>
          <label className="block text-sm font-medium text-ink2">Indexing speed</label>
          <p className="mt-1 text-xs text-ink4">
            How hard PM works your machine while it indexes <strong>Drive files and imports</strong>{" "}
            (email later). Calendars are tiny and always sync at full speed.
          </p>
        </div>
        <SegmentedControl
          className="mt-0.5 shrink-0"
          value={value}
          onChange={(v) => onChange(v as "fast" | "gentle")}
          options={[
            { value: "fast", label: "Fast" },
            { value: "gentle", label: "Gentle" },
          ]}
        />
      </div>
      <Collapsible
        className="mt-2.5"
        defaultOpen={showPower}
        title={<span className="text-xs text-ink3">What do Fast and Gentle do?</span>}
      >
        <dl className="mt-1.5 space-y-1.5 text-xs leading-relaxed text-ink4">
          <div>
            <dt className="inline font-medium text-ink3">Fast</dt>
            <dd className="inline">
              {" "}
              — indexes at full speed, using as much CPU and memory as it needs. Best on a capable
              machine, or when you just want it finished.
            </dd>
          </div>
          <div>
            <dt className="inline font-medium text-ink3">Gentle</dt>
            <dd className="inline">
              {" "}
              — pauses briefly between files and embeds in smaller batches, so it uses less CPU and
              less memory and your computer stays responsive. Best for slower or low-memory
              machines, or when you’re working while it runs. Indexing takes longer.
            </dd>
          </div>
        </dl>
        <p className="mt-2 text-xs text-faint">
          Changes apply right away — even partway through a sync.
        </p>
      </Collapsible>
    </div>
  );
}

function Divider() {
  return <div className="my-3 border-t border-border" />;
}

/** A visible-but-disabled placeholder for a not-yet-built provider connection. */
function ComingSoonRow({ name, detail }: { name: string; detail?: string }) {
  return (
    <div className="flex items-center justify-between gap-2 py-1.5 opacity-60" aria-disabled="true">
      <div>
        <div className="text-sm text-ink2">{name}</div>
        {detail && <p className="mt-0.5 text-xs text-ink4">{detail}</p>}
      </div>
      <span className="shrink-0 rounded-[var(--radius-sm)] border border-border px-1.5 py-0.5 font-mono text-[10px] uppercase tracking-wide text-ink4">
        coming soon
      </span>
    </div>
  );
}
