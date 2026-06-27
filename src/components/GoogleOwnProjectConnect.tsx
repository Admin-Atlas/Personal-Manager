// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useState } from "react";
import { connectDrive, connectGoogleCalendarAccount } from "../lib/ipc";
import { Button, Input } from "./ui";

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
 * covers the other.
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
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      const id = clientId.trim();
      const secret = clientSecret.trim();
      if (service === "drive") await connectDrive(id, secret);
      else await connectGoogleCalendarAccount(id, secret);
      setClientId("");
      setClientSecret("");
      setOpen(false);
      await onConnected();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

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
            className="disabled:opacity-40"
          >
            {busy ? "Waiting for Google…" : "Connect with this project"}
          </Button>
          {error && (
            <p
              className="rounded-[var(--radius)] px-2 py-1.5 text-xs text-st-due"
              style={{ background: "color-mix(in oklab, var(--st-due) 15%, transparent)" }}
            >
              {error}
            </p>
          )}
        </div>
      )}
    </div>
  );
}
