// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { ReactNode } from "react";
import { useDepth } from "../theme";
import { Collapsible } from "./ui";
import { GoogleCalendarConnection } from "./GoogleCalendarConnection";
import { GoogleDriveConnection } from "./GoogleDriveConnection";
import { IcsFeedSubscription } from "./IcsFeedSubscription";

/**
 * The **Connectors** hub (Stage 3, §8.1) — where external accounts are connected so the
 * Archivist can index them, and where the read-only calendar now lives (the standalone Calendar
 * tab was folded in here).
 *
 * Grouped **by service** (Calendar / Drive / Email), each listing its provider connections
 * (Google available now; Apple / Microsoft coming-soon). A provider's BYO-OAuth client is
 * provider-level and shared across its services — it's set up inside the first Google service you
 * connect (see {@link GoogleCredentialBlock}) and reused everywhere. Every connection is
 * **independently opt-in and removable** — subscribe to a calendar URL only, connect Drive only,
 * or set up the OAuth client and connect nothing yet; nothing cascades or auto-enables.
 *
 * Calendar (Google OAuth + the zero-auth iCal subscription) and Drive (Google Drive, index-only)
 * are live; Microsoft, Apple, and Email show as coming-soon placeholders that drop into their
 * service sections as later cards land — no change to the surrounding structure.
 */
export function ConnectorsSettings() {
  return (
    <div className="mt-5 border-t border-border pt-4" data-help="settings-connectors">
      <label className="block text-sm font-medium text-ink2">Connectors</label>
      <p className="mt-1 text-xs text-ink4">
        Connect external accounts so PM can find and use what’s in them, grouped by what they do.
        Every connection is independently opt-in and removable — nothing cascades. Credentials and
        tokens live only in your keychain.
      </p>

      <ServiceSection
        title="Calendar"
        blurb="Mirror your agenda (read-only) for the focus view, chat, and the “Due soon” status."
      >
        <GoogleCalendarConnection />
        <Divider />
        <IcsFeedSubscription />
        <Divider />
        <ComingSoonRow name="Apple Calendar" />
        <ComingSoonRow name="Outlook Calendar" />
      </ServiceSection>

      <ServiceSection
        title="Drive"
        blurb="Index your cloud files so PM can find them and ground answers in their contents."
      >
        <GoogleDriveConnection />
        <Divider />
        <ComingSoonRow name="OneDrive" detail="Microsoft — the connector after Drive." />
        <ComingSoonRow name="iCloud Drive" detail="Apple — coming later." />
      </ServiceSection>

      <ServiceSection
        title="Email"
        blurb="Index your mail so PM can search it and reference it in chat."
      >
        <ComingSoonRow name="Gmail" />
        <ComingSoonRow name="Outlook mail" />
      </ServiceSection>
    </div>
  );
}

/**
 * A capability group (Calendar / Drive / Email) holding one or more provider connections. These
 * sections get large (multi-account Drive, calendar feeds), so each is **collapsible**: it opens by
 * default for Standard/Power density and starts **collapsed at Minimal** (so the tab stays scannable),
 * and can always be toggled either way. The disclosure state is per-section local UI.
 */
function ServiceSection({
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
