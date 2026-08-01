// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The vault open-error gate (spec §2; B1-6): shown when the store *failed to open* at boot.
// The carried `VaultFault` decides the story and the actions (issue #343):
//
//  - "denied"   — Windows is refusing this account access to the (usually shared) folder.
//                 The data is intact; Repair access is the primary action, never deletion.
//  - "no-vault"/"not-found" — the pointed folder no longer holds a vault (moved, deleted,
//                 or an unplugged drive). Try again / step back to a vault on this account.
//  - anything else — the transient story as before (antivirus or Windows Search holding
//                 the file, disk I/O): Retry, with db::open's friendly message.
//
// It also offers a last-resort "Start fresh": if the store genuinely can't be opened (its key was
// lost — e.g. an interrupted "Remove PM data" — so a fresh boot key can't decrypt it, and Retry
// loops forever), this deletes the unreadable store and relaunches into a clean first-run, so the
// user is never trapped. Guarded by a type-to-confirm because it permanently discards the vault —
// and refused outright by the backend for pointed or access-denied vaults (denied is never a brick).

import { useState } from "react";
import { relaunch } from "@tauri-apps/plugin-process";
import { detachFromSharedVault, resetAfterOpenError, retryOpenVault } from "../lib/ipc";
import type { VaultStatus } from "../lib/types";
import { Button, Callout, Input } from "./ui";
import { DetachConfirm, RepairAccessButton } from "./VaultRecovery";

/** The phrase the user types to arm the destructive "Start fresh" recovery. */
const RESET_PHRASE = "Start fresh";

export function VaultOpenError({
  status,
  onResolved,
}: {
  status: VaultStatus;
  onResolved: () => void;
}) {
  const fault = status.fault;
  const denied = fault?.code === "denied";
  const gone = fault?.code === "no-vault" || fault?.code === "not-found";
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(fault?.message ?? null);
  // The destructive escape hatch, revealed only if the user asks for it.
  const [showReset, setShowReset] = useState(false);
  const [confirmText, setConfirmText] = useState("");
  const [resetting, setResetting] = useState(false);
  const [confirmDetach, setConfirmDetach] = useState(false);

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
  // (the pointer is retired, not erased — the shared folder itself is untouched and
  // rejoinable) and reboot the webview on it. Confirmed via DetachConfirm, which states
  // whether a set-aside vault or a fresh empty one is on the other side (issue #337/#343).
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
        <h1 className="font-ui text-lg font-semibold text-ink">
          {denied ? "PM can't reach your vault right now" : "Couldn't open your vault"}
        </h1>
        <p className="mt-1 max-w-xs text-sm text-ink4">
          {denied
            ? "Windows is refusing this account access to the vault folder. Your documents " +
              "are still there and still encrypted — nothing has been deleted."
            : gone
              ? "The folder PM points at doesn't hold a vault any more — it may have been " +
                "moved or deleted, or be on a drive that isn't connected."
              : "Your documents are safe — the vault file was momentarily unavailable, often " +
                "because antivirus or Windows Search was scanning it. Try again in a moment."}
        </p>
      </div>

      {error && (
        <Callout as="p" className="max-w-xs break-words">
          {error}
        </Callout>
      )}

      {/* Denied leads with Repair (the data is fine; the permissions are the problem). */}
      {denied && <RepairAccessButton path={fault?.path ?? null} onRepaired={onResolved} />}

      <Button
        variant={denied ? "secondary" : "primary"}
        disabled={busy || resetting}
        onClick={() => void retry()}
      >
        {busy ? "Trying again…" : "Try again"}
      </Button>

      {status.pointed_root && (
        <div className="max-w-xs">
          <Button
            variant="secondary"
            disabled={busy || resetting}
            onClick={() => setConfirmDetach(true)}
          >
            Use a vault on this account instead
          </Button>
          <p className="mt-1 text-xs text-ink4">
            PM points at a shared folder right now. This steps back to a vault of your own — the
            shared folder isn't touched, and you can rejoin it any time from Settings.
          </p>
        </div>
      )}

      {/* The destructive "Start fresh" recovery is only for THIS profile's own vault. A pointed
          (shared/joined) vault belongs to a folder we don't own, and a DENIED vault is intact
          data behind a permissions problem — the backend refuses both; the UI doesn't offer
          them in the first place. */}
      {status.pointed_root || denied ? null : !showReset ? (
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
              variant="danger"
              disabled={resetting || confirmText !== RESET_PHRASE}
              onClick={() => void startFresh()}
            >
              {resetting ? "Starting fresh…" : "Delete vault & start fresh"}
            </Button>
          </div>
        </div>
      )}

      <DetachConfirm
        open={confirmDetach}
        onClose={() => setConfirmDetach(false)}
        status={status}
        onConfirm={() => void detach()}
      />

      {status.location && <p className="max-w-xs break-all text-xs text-ink4">{status.location}</p>}
    </div>
  );
}
