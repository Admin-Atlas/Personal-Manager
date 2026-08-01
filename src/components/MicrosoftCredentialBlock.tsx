// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useState } from "react";
import { clearMicrosoftClient, setMicrosoftClient } from "../lib/ipc";
import { useBusyRun } from "../lib/useBusyRun";
import { Button, Callout, ConfirmDialog, Input } from "./ui";

/**
 * The shared **BYO Microsoft OAuth client** credential block — one Microsoft Entra "Mobile & desktop"
 * app registration the user pastes once and that OneDrive (and any future Microsoft service) reuses.
 * Unlike Google's block this is a **public client**: there is no secret to copy — just the
 * Application (client) ID. It is **provider-level**: `microsoft::has_client()` is a single global
 * flag, so clearing it signs every Microsoft service out.
 *
 * Rendered inside the OneDrive block when it isn't yet usable: `configured=false` shows the setup
 * wizard + paste form; `configured=true` shows a one-line confirmed state with a Clear action.
 * `onChange` refreshes the host after a save/clear. PM ships no Microsoft secret — there isn't one;
 * the client id lives only in the keychain.
 */
export function MicrosoftCredentialBlock({
  configured,
  onChange,
}: {
  configured: boolean;
  onChange: () => void | Promise<void>;
}) {
  const [clientId, setClientId] = useState("");
  const [showSetup, setShowSetup] = useState(false);
  const { busy, error, run } = useBusyRun();
  const [confirmClear, setConfirmClear] = useState(false);

  // The host refresh rides inside the mutation so its failure surfaces on the same error line.
  const saveCreds = () =>
    run("save", async () => {
      await setMicrosoftClient(clientId.trim());
      setClientId("");
      await onChange();
    });

  const clearCreds = () =>
    run("clear", async () => {
      await clearMicrosoftClient();
      await onChange();
    });

  return (
    <div data-help="connectors-microsoft-client">
      <div className="flex items-center justify-between gap-2">
        <span className="text-sm font-medium text-ink">Microsoft sign-in (one-time setup)</span>
        {configured && (
          <span className="inline-flex shrink-0 items-center gap-1.5 text-xs text-st-quick">
            <span className="h-1.5 w-1.5 rounded-full bg-[var(--st-quick)]" /> Configured
          </span>
        )}
      </div>
      <p className="mt-1 text-xs text-ink4">
        One Microsoft Entra “Mobile &amp; desktop” app registration, pasted once and shared by every
        Microsoft service (OneDrive). It’s a desktop app, so there’s <em>no secret</em> — just the
        client ID; it stays in your keychain. Setting it up connects nothing on its own.
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
              placeholder="Application (client) ID"
            />
            <Button onClick={saveCreds} disabled={busy != null || !clientId.trim()}>
              {busy === "save" ? "Saving…" : "Save client ID"}
            </Button>
          </div>
        </>
      ) : (
        <div className="mt-2">
          <Button
            variant="tertiary"
            size="sm"
            onClick={() => setConfirmClear(true)}
            disabled={busy != null}
          >
            Clear client ID
          </Button>
        </div>
      )}

      {error && (
        <Callout as="p" className="mt-2">
          {error}
        </Callout>
      )}

      <ConfirmDialog
        open={confirmClear}
        title="Clear Microsoft client ID?"
        danger
        confirmLabel="Clear"
        onConfirm={() => {
          setConfirmClear(false);
          void clearCreds();
        }}
        onClose={() => setConfirmClear(false)}
      >
        This forgets your Microsoft client ID and signs out every connected OneDrive account,
        clearing its mirrored data. Indexed items are kept and stay findable until then. You can
        re-enter the client ID anytime.
      </ConfirmDialog>
    </div>
  );
}

/** Step-by-step for creating a BYO Microsoft Entra "Mobile & desktop" public OAuth client. */
function ClientSetupGuide() {
  const link = "text-accent-text underline hover:brightness-110";
  return (
    <ol className="mt-2 space-y-1 rounded-[var(--radius)] bg-surface px-3 py-2 text-xs text-ink3">
      <li>
        1. Open{" "}
        <a
          href="https://entra.microsoft.com/#view/Microsoft_AAD_RegisteredApps/ApplicationsListBlade"
          target="_blank"
          rel="noreferrer"
          className={link}
        >
          Entra ID → App registrations
        </a>{" "}
        and choose <span className="text-ink2">New registration</span>.
      </li>
      <li>
        2. Name it anything (e.g. “PM”). Under{" "}
        <span className="text-ink2">Supported account types</span> choose{" "}
        <span className="text-ink2">
          Accounts in any organizational directory and personal Microsoft accounts
        </span>{" "}
        — that’s what lets both work/school and personal OneDrive sign in.
      </li>
      <li>
        3. Under <span className="text-ink2">Redirect URI</span>, pick platform{" "}
        <span className="text-ink2">Public client/native (mobile &amp; desktop)</span> and enter{" "}
        <span className="font-mono text-[0.6875rem] text-ink2">http://127.0.0.1</span>. (You can
        also do this later under{" "}
        <span className="text-ink2">
          Authentication → Add a platform → Mobile and desktop applications
        </span>
        .) Register.
      </li>
      <li>
        4. On the app’s <span className="text-ink2">Overview</span>, copy the{" "}
        <span className="text-ink2">Application (client) ID</span> and paste it below. There is no
        secret to create — leave it as a public client.
      </li>
      <li className="text-ink4">
        If a sign-in fails: <span className="text-ink2">“need admin approval / AADSTS65001”</span>{" "}
        means a work/school tenant requires an admin to approve{" "}
        <span className="text-ink2">Files.Read</span> — ask your admin, or use a personal Microsoft
        account instead. The “unverified app” notice is expected — continue past it. If the browser
        can’t reach the sign-in page, double-check the redirect URI is exactly{" "}
        <span className="font-mono text-[0.6875rem] text-ink2">http://127.0.0.1</span> under Mobile
        &amp; desktop.
      </li>
    </ol>
  );
}
