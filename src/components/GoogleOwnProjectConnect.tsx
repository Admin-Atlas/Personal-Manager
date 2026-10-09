// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useState } from "react";
import { connectDrive, connectGoogleCalendarAccount, googleSavedProjects } from "../lib/ipc";
import { Button, Callout, Input } from "./ui";

/**
 * **Connect a Google account with its OWN Cloud project** — the Advanced-Protection path.
 *
 * Most accounts share the one BYO client set up at the group level. But a Google account enrolled in
 * **Advanced Protection** can't authorise a shared third-party project (Google hard-blocks it); it can
 * only sign in with a client from a project the account itself owns. And two such accounts can't share
 * one project, so each needs its own. This disclosure lets the user paste a per-account Client ID +
 * secret for that account's sign-in; the backend remembers it (keyed by the account's email) so every
 * later token refresh reuses it. Sits under the normal "Add another account" button in both the Drive
 * and Calendar connectors — same Google account identity, so a project entered for one service also
 * covers the other: an account whose project is already saved is offered as a one-click "use its
 * saved project" sign-in, with no pasting.
 *
 * Self-contained: it runs the connect itself (`connectDrive` / `connectGoogleCalendarAccount` with the
 * creds) and calls `onConnected` so the host refreshes its account list (and, for Calendar, syncs).
 */
export function GoogleOwnProjectConnect({
  service,
  disabled = false,
  onConnected,
}: {
  service: "drive" | "calendar";
  disabled?: boolean;
  onConnected: () => void | Promise<void>;
}) {
  const [open, setOpen] = useState(false);
  const [clientId, setClientId] = useState("");
  const [clientSecret, setClientSecret] = useState("");
  // Which sign-in is waiting on Google: the pasted project, or a saved one (by email). Its own button
  // says "Waiting for Google…"; every button is disabled meanwhile.
  const [busyFor, setBusyFor] = useState<"paste" | string | null>(null);
  const busy = busyFor != null;
  const [error, setError] = useState<string | null>(null);
  // Accounts whose own project PM already holds — offered as one-click sign-ins. Re-read whenever the
  // panel opens and whenever the host card finishes an action (`disabled` falls back to false), so a
  // project forgotten or saved by a connect or disconnect since is reflected. A button that still went
  // stale (the other Google card changed it) is refused by the backend with a clear message rather
  // than signing in some other way.
  const [saved, setSaved] = useState<string[]>([]);

  useEffect(() => {
    if (!open || disabled) return;
    let live = true;
    googleSavedProjects()
      .then((emails) => live && setSaved(emails))
      .catch(() => live && setSaved([]));
    return () => {
      live = false;
    };
  }, [open, disabled]);

  const connectWith = async (which: "paste" | string, connect: () => Promise<unknown>) => {
    setBusyFor(which);
    setError(null);
    try {
      await connect();
      setClientId("");
      setClientSecret("");
      setOpen(false);
      await onConnected();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusyFor(null);
    }
  };

  const submit = () => {
    const id = clientId.trim();
    const secret = clientSecret.trim();
    return connectWith("paste", () =>
      service === "drive" ? connectDrive(id, secret) : connectGoogleCalendarAccount(id, secret),
    );
  };

  const connectSaved = (email: string) =>
    connectWith(email, () =>
      service === "drive"
        ? connectDrive(undefined, undefined, email)
        : connectGoogleCalendarAccount(undefined, undefined, email),
    );

  return (
    <div className="mt-2">
      <button
        type="button"
        onClick={() => setOpen((o) => !o)}
        className="text-xs text-accent-text hover:brightness-110"
      >
        {open ? "Hide own-project sign-in" : "Advanced Protection account? Use its own project →"}
      </button>
      {open && (
        <div className="mt-2 space-y-2 rounded-[var(--radius)] border border-border p-3">
          <p className="text-xs text-ink4">
            A Google account with <span className="text-ink2">Advanced Protection</span> can’t use a
            shared project — it must sign in with a Cloud project it owns (and two such accounts
            can’t share one). Paste that project’s <span className="text-ink2">Desktop app</span>{" "}
            Client ID + secret; PM remembers it for this account only.
          </p>
          {saved.length > 0 && (
            <div className="space-y-1">
              <p className="text-xs text-ink3">
                Already saved a project for one of these accounts?
              </p>
              <div className="flex flex-wrap gap-2">
                {saved.map((email) => (
                  <Button
                    key={email}
                    variant="secondary"
                    onClick={() => void connectSaved(email)}
                    disabled={disabled || busy}
                  >
                    {busyFor === email ? "Waiting for Google…" : <>Use {email}&rsquo;s project</>}
                  </Button>
                ))}
              </div>
              <p className="text-xs text-ink4">Or paste another account&rsquo;s project:</p>
            </div>
          )}
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
            onClick={submit}
            disabled={disabled || busy || !clientId.trim() || !clientSecret.trim()}
          >
            {busyFor === "paste" ? "Waiting for Google…" : "Connect with this project"}
          </Button>
          {error && <Callout as="p">{error}</Callout>}
        </div>
      )}
    </div>
  );
}
