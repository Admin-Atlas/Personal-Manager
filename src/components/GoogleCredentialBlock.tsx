// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useState } from "react";
import { clearGoogleClient, setGoogleClient } from "../lib/ipc";
import { useBusyRun } from "../lib/useBusyRun";
import { Button, ConfirmDialog, Input } from "./ui";

/**
 * The shared **BYO Google OAuth client** credential block — one Google Cloud "Desktop app"
 * client (id + secret) the user pastes once and that every Google service reuses (Calendar
 * now, Drive in PR2, Gmail later). It is **provider-level**: `google::has_client()` is a single
 * global flag, so configuring it from any Google service block enables them all, and clearing it
 * signs every Google service out.
 *
 * Rendered inside a Google service block when that service isn't yet usable: `configured=false`
 * shows the setup wizard + paste form; `configured=true` shows a one-line confirmed state with a
 * Clear action. `onChange` refreshes the host after a save/clear. PM ships no Google secret
 * (rule #1) — the credentials live only in the keychain.
 */
export function GoogleCredentialBlock({
  configured,
  onChange,
}: {
  configured: boolean;
  onChange: () => void | Promise<void>;
}) {
  const [clientId, setClientId] = useState("");
  const [clientSecret, setClientSecret] = useState("");
  const [showSetup, setShowSetup] = useState(false);
  const { busy, error, run } = useBusyRun();
  const [confirmClear, setConfirmClear] = useState(false);

  // The host refresh rides inside the mutation so its failure surfaces on the same error line.
  const saveCreds = () =>
    run("save", async () => {
      await setGoogleClient(clientId.trim(), clientSecret.trim());
      setClientId("");
      setClientSecret("");
      await onChange();
    });

  const clearCreds = () =>
    run("clear", async () => {
      await clearGoogleClient();
      await onChange();
    });

  return (
    <div data-help="connectors-google-client">
      <div className="flex items-center justify-between gap-2">
        <span className="text-sm font-medium text-ink">Google sign-in (one-time setup)</span>
        {configured && (
          <span className="inline-flex shrink-0 items-center gap-1.5 text-xs text-st-quick">
            <span className="h-1.5 w-1.5 rounded-full bg-[var(--st-quick)]" /> Configured
          </span>
        )}
      </div>
      <p className="mt-1 text-xs text-ink4">
        One Google Cloud “Desktop app” client, pasted once and shared by every Google service
        (Calendar, Drive). PM ships no Google secret — you supply your own; it stays in your
        keychain. Setting it up connects nothing on its own.
      </p>

      {!configured ? (
        <>
          <button
            onClick={() => setShowSetup((s) => !s)}
            className="mt-1 text-xs text-accent-text hover:brightness-110"
          >
            {showSetup ? "Hide setup steps" : "How do I create this? →"}
          </button>
          {showSetup && <ClientSetupGuide />}
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
        </>
      ) : (
        <div className="mt-2">
          <Button
            variant="tertiary"
            onClick={() => setConfirmClear(true)}
            disabled={busy != null}
            className="px-2 py-1.5 text-xs"
          >
            Clear credentials
          </Button>
        </div>
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
        open={confirmClear}
        title="Clear Google credentials?"
        danger
        confirmLabel="Clear"
        onConfirm={() => {
          setConfirmClear(false);
          void clearCreds();
        }}
        onClose={() => setConfirmClear(false)}
      >
        This forgets your Google client ID + secret and signs out every connected Google service
        (Calendar, Drive), clearing its mirrored data. Calendar subscriptions (iCal) are unaffected.
        You can re-enter the credentials anytime.
      </ConfirmDialog>
    </div>
  );
}

/** Step-by-step for creating a BYO Google Cloud "Desktop app" OAuth client. */
function ClientSetupGuide() {
  const link = "text-accent-text underline hover:brightness-110";
  return (
    <ol className="mt-2 space-y-1 rounded-[var(--radius)] bg-surface px-3 py-2 text-xs text-ink3">
      <li>
        1. Open the{" "}
        <a
          href="https://console.cloud.google.com/projectcreate"
          target="_blank"
          rel="noreferrer"
          className={link}
        >
          Google Cloud Console
        </a>{" "}
        and create a project (or reuse one).
      </li>
      <li>
        2. <span className="text-ink2">Enable the API</span> for each service you’ll use —{" "}
        <a
          href="https://console.cloud.google.com/apis/library/drive.googleapis.com"
          target="_blank"
          rel="noreferrer"
          className={link}
        >
          Google Drive API
        </a>{" "}
        ,{" "}
        <a
          href="https://console.cloud.google.com/apis/library/calendar-json.googleapis.com"
          target="_blank"
          rel="noreferrer"
          className={link}
        >
          Google Calendar API
        </a>{" "}
        and/or{" "}
        <a
          href="https://console.cloud.google.com/apis/library/sheets.googleapis.com"
          target="_blank"
          rel="noreferrer"
          className={link}
        >
          Google Sheets API
        </a>{" "}
        (select your project, then <span className="text-ink2">Enable</span>). Sheets is what lets
        Drive read a spreadsheet’s tab names and column headers rather than just its filename.
        Skipping this is what causes the “this API has not been used / is disabled” error.
      </li>
      <li>
        3. Configure the{" "}
        <a
          href="https://console.cloud.google.com/auth/audience"
          target="_blank"
          rel="noreferrer"
          className={link}
        >
          OAuth consent screen
        </a>{" "}
        (User type <span className="text-ink2">External</span>), then under{" "}
        <span className="text-ink2">Audience → Test users</span> add the exact Google account you’ll
        sign in with.
      </li>
      <li>
        4. Under{" "}
        <a
          href="https://console.cloud.google.com/apis/credentials"
          target="_blank"
          rel="noreferrer"
          className={link}
        >
          Credentials
        </a>{" "}
        → <span className="text-ink2">Create credentials</span> →{" "}
        <span className="text-ink2">OAuth client ID</span>, choose application type{" "}
        <span className="text-ink2">Desktop app</span>.
      </li>
      <li>5. Copy the Client ID + Client secret and paste them below.</li>
      <li>
        6. <span className="text-ink2">More than one Google account?</span> You don’t need a new
        project or new credentials — reuse the same Client ID + secret (it stays saved here, shared
        by every account). For each extra account, add its email under{" "}
        <span className="text-ink2">Audience → Test users</span> (step 3), then in PM use{" "}
        <span className="text-ink2">Add another account</span> and sign in with it. (A separate
        project is only needed if you’d rather keep accounts fully isolated.)
      </li>
      <li className="text-ink4">
        If a sign-in fails:{" "}
        <span className="text-ink2">“…API has not been used… or is disabled” (403)</span> means you
        skipped step 2 — enable it, then wait ~1 minute.{" "}
        <span className="text-ink2">“Access blocked… developer-approved testers” (403)</span> means
        the signed-in account isn’t under <span className="text-ink2">Audience → Test users</span>{" "}
        (step 3). The “unverified app” screen is expected — continue past it. Test mode signs you
        out every 7 days; to avoid that you can <span className="text-ink2">Publish app</span>{" "}
        (Audience → Publish app), which makes your consent screen public — though only your saved
        client secret can actually use it. <span className="text-ink2">Advanced Protection</span>{" "}
        accounts can’t use a shared project at all — connect each one with{" "}
        <span className="text-ink2">“Advanced Protection account? Use its own project”</span> under
        the Drive/Calendar connect button, pasting a Client ID + secret from a project that account
        owns (one project per such account; it can’t be shared).
      </li>
    </ol>
  );
}
