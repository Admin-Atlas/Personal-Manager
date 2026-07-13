// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The vault open-error gate (spec §2; B1-6): shown when the store *failed to open* at boot
// from a transient condition — antivirus or Windows Search momentarily holding the file, or a
// disk I/O blip — rather than being locked. It offers Retry (db::open's message is already
// friendly and retryable) so a passing hiccup no longer aborts the whole app. Distinct from
// VaultUnlock: there is nothing to type here — the key is fine, the file was just unavailable.
//
// It also offers a last-resort "Start fresh": if the store genuinely can't be opened (its key was
// lost — e.g. an interrupted "Remove PM data" — so a fresh boot key can't decrypt it, and Retry
// loops forever), this deletes the unreadable store and relaunches into a clean first-run, so the
// user is never trapped. Guarded by a type-to-confirm because it permanently discards the vault.

import { useState } from "react";
import { relaunch } from "@tauri-apps/plugin-process";
import { detachFromSharedVault, resetAfterOpenError, retryOpenVault } from "../lib/ipc";
import type { VaultStatus } from "../lib/types";
import { Button, Input } from "./ui";

/** The phrase the user types to arm the destructive "Start fresh" recovery. */
const RESET_PHRASE = "Start fresh";

export function VaultOpenError({
  status,
  onResolved,
}: {
  status: VaultStatus;
  onResolved: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(status.open_error);
  // The destructive escape hatch, revealed only if the user asks for it.
  const [showReset, setShowReset] = useState(false);
  const [confirmText, setConfirmText] = useState("");
  const [resetting, setResetting] = useState(false);

  async function retry() {
    if (busy || resetting) return;
    setBusy(true);
    setError(null);
    try {
      await retryOpenVault();
      onResolved(); // reload the deferred boot state; this gate unmounts once the store opens
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  }

  // A joined/moved vault that stopped answering: step back to a vault on this account
  // (clear the pointer — the shared folder itself is untouched) and reboot the webview
  // on it. The safe exit for a joiner whose access was revoked or whose owner made the
  // vault private (issue #337).
  async function detach() {
    if (busy || resetting) return;
    setBusy(true);
    setError(null);
    try {
      await detachFromSharedVault();
      window.location.reload();
    } catch (e) {
      setError(String(e));
      setBusy(false);
    }
  }

  // Delete the unreadable store and relaunch into a clean first-run. Permanent — gated by the
  // type-to-confirm above the button — but the only way out when the store can't be decrypted.
  async function startFresh() {
    if (resetting || confirmText !== RESET_PHRASE) return;
    setResetting(true);
    setError(null);
    try {
      await resetAfterOpenError();
      await relaunch();
    } catch (e) {
      setError(String(e));
      setResetting(false);
    }
  }

  return (
    <div className="flex h-full flex-col items-center justify-center gap-4 bg-bg px-6 text-center">
      <div className="flex h-12 w-12 items-center justify-center rounded-full border border-border text-ink2">
        {/* Warning-triangle glyph — no icon dependency (matches the other vault gates). */}
        <svg width="22" height="22" viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <path
            d="M12 4 3 19h18L12 4Z"
            stroke="currentColor"
            strokeWidth="1.6"
            strokeLinejoin="round"
          />
          <path d="M12 10v4" stroke="currentColor" strokeWidth="1.6" strokeLinecap="round" />
          <circle cx="12" cy="16.6" r="0.9" fill="currentColor" />
        </svg>
      </div>

      <div>
        <h1 className="font-ui text-lg font-semibold text-ink">Couldn't open your vault</h1>
        <p className="mt-1 max-w-xs text-sm text-ink4">
          Your documents are safe — the vault file was momentarily unavailable, often because
          antivirus or Windows Search was scanning it. Try again in a moment.
        </p>
      </div>

      {error && (
        <p
          className="max-w-xs break-words rounded-[var(--radius)] px-3 py-2 text-xs text-st-due"
          style={{ background: "color-mix(in oklab, var(--st-due) 15%, transparent)" }}
        >
          {error}
        </p>
      )}

      <Button variant="primary" disabled={busy || resetting} onClick={() => void retry()}>
        {busy ? "Trying again…" : "Try again"}
      </Button>

      {status.pointed_root && (
        <div className="max-w-xs">
          <Button variant="secondary" disabled={busy || resetting} onClick={() => void detach()}>
            Use a vault on this account instead
          </Button>
          <p className="mt-1 text-xs text-ink4">
            PM points at a shared folder right now. This steps back to a vault of your own — the
            shared folder isn't touched, and you can rejoin it any time from Settings.
          </p>
        </div>
      )}

      {/* The destructive "Start fresh" recovery is only for THIS profile's own vault. A pointed
          (shared/joined) vault belongs to a folder we don't own — deleting it could destroy
          another account's data — so it's hidden there; detach above is the way out. */}
      {status.pointed_root ? null : !showReset ? (
        <button
          className="text-xs text-ink4 underline underline-offset-2 hover:text-ink3"
          disabled={busy || resetting}
          onClick={() => {
            setError(null);
            setShowReset(true);
          }}
        >
          Still won't open?
        </button>
      ) : (
        <div className="w-full max-w-xs rounded-[var(--radius)] border border-border bg-surface p-3 text-left">
          <p className="text-xs text-ink3">
            Try “Try again” a few times first — a vault that&apos;s only momentarily locked (often
            by antivirus or Windows Search) opens once the file is free. If it truly never opens,
            the vault is damaged and can&apos;t be recovered on this device. You can start fresh —
            this <span className="font-medium text-st-due">permanently deletes the vault</span> and
            sets PM up again from scratch. Your saved keys and sign-ins are kept.
          </p>
          <p className="mt-2 text-xs text-ink4">
            Type <span className="font-mono font-medium text-ink2">{RESET_PHRASE}</span> to confirm.
          </p>
          <Input
            value={confirmText}
            onChange={(e) => setConfirmText(e.target.value)}
            placeholder={RESET_PHRASE}
            autoComplete="off"
            className="mt-2"
            disabled={resetting}
          />
          <div className="mt-3 flex justify-end gap-2">
            <Button
              variant="tertiary"
              disabled={resetting}
              onClick={() => {
                setShowReset(false);
                setConfirmText("");
              }}
            >
              Cancel
            </Button>
            <Button
              variant="primary"
              disabled={resetting || confirmText !== RESET_PHRASE}
              onClick={() => void startFresh()}
              style={
                confirmText === RESET_PHRASE
                  ? {
                      background: "color-mix(in oklab, var(--st-due) 15%, transparent)",
                      color: "var(--st-due)",
                    }
                  : undefined
              }
            >
              {resetting ? "Starting fresh…" : "Delete vault & start fresh"}
            </Button>
          </div>
        </div>
      )}

      {status.location && (
        <p className="max-w-xs break-all text-xs text-faint">{status.location}</p>
      )}
    </div>
  );
}
