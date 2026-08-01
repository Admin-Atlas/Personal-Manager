// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The vault unlock gate (spec §2): shown before the main app when a passphrase-protected
// vault boots without a cached key on this profile — a second OS profile opening a shared
// vault, or after "forget passphrase here". Unlike the biometric LockScreen, this gates
// *real* decryption (the DB key is derived from the passphrase, so the store can't open
// without it), which is exactly why there is no "open anyway" escape: there's no backdoor.

import { useState } from "react";
import { detachFromSharedVault, unlockVault, vaultFaultOf } from "../lib/ipc";
import type { VaultStatus } from "../lib/types";
import { paddedPassphraseHint } from "../lib/vaultPassphrase";
import { Button, Callout, Input, useFieldA11y } from "./ui";
import { DetachConfirm, RepairAccessButton } from "./VaultRecovery";

export function VaultUnlock({
  status,
  onUnlocked,
}: {
  status: VaultStatus | null;
  onUnlocked: () => void;
}) {
  const [pass, setPass] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // An unlock that failed because the FOLDER is refusing access, not because the
  // passphrase is wrong — the retype-forever trap from the lockout incident. Branch on
  // the classified code, never on message text, and lead with Repair instead.
  const [deniedPath, setDeniedPath] = useState<string | null>(null);
  const [confirmDetach, setConfirmDetach] = useState(false);
  // Name the passphrase field (placeholder is not an accessible name) and tie the error to it so it's
  // both associated and announced — with a visually-hidden label, so the centered gate looks unchanged.
  const field = useFieldA11y({ error });

  async function unlock() {
    if (busy || pass.trim().length === 0) return;
    setBusy(true);
    setError(null);
    setDeniedPath(null);
    try {
      await unlockVault(pass);
      onUnlocked(); // this gate unmounts on success
    } catch (e) {
      const fault = vaultFaultOf(e);
      if (fault?.code === "denied") {
        setDeniedPath(fault.path ?? status?.location ?? null);
        setError(
          "It's not the passphrase — Windows is refusing this account access to the vault folder.",
        );
      } else if (fault?.code === "wrong-passphrase") {
        setError(
          "That passphrase doesn't match this vault. If it was changed, check with whoever set it." +
            paddedPassphraseHint(pass),
        );
      } else {
        setError(String(e));
      }
      setBusy(false);
    }
  }

  // A joined (pointed) vault the user can't unlock — the owner changed the passphrase and didn't
  // share it, or revoked access — needs a way out that isn't "guess forever". Step back to a vault
  // of your own (confirmed via DetachConfirm — it says exactly which vault is on the other side);
  // the shared folder is untouched and can be rejoined later.
  async function detach() {
    if (busy) return;
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

  return (
    <div className="flex h-full flex-col items-center justify-center gap-4 bg-bg px-6 text-center">
      <div className="flex h-12 w-12 items-center justify-center rounded-full border border-border text-ink2">
        {/* Padlock glyph — no icon dependency (matches LockScreen). */}
        <svg width="22" height="22" viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <rect
            x="4"
            y="10"
            width="16"
            height="10"
            rx="2"
            stroke="currentColor"
            strokeWidth="1.6"
          />
          <path
            d="M8 10V7a4 4 0 1 1 8 0v3"
            stroke="currentColor"
            strokeWidth="1.6"
            strokeLinecap="round"
          />
        </svg>
      </div>

      <div>
        <h1 className="font-ui text-lg font-semibold text-ink">Unlock your vault</h1>
        <p className="mt-1 max-w-xs text-sm text-ink4">
          This vault is protected by a passphrase. Enter it to open your documents on this device —
          it's cached here afterwards, so you're only asked once.
        </p>
      </div>

      <form
        onSubmit={(e) => {
          e.preventDefault();
          void unlock();
        }}
        className="flex w-full max-w-xs flex-col gap-2"
      >
        <label className="sr-only" {...field.labelProps}>
          Passphrase
        </label>
        <Input
          type="password"
          autoComplete="current-password"
          autoFocus
          placeholder="Passphrase"
          value={pass}
          onChange={(e) => setPass(e.target.value)}
          disabled={busy}
          {...field.controlProps}
        />
        {/* `live={false}` because `errorProps` is the authority here: it carries both the
            `role="alert"` AND the id that the control's `aria-describedby` points at. */}
        {error && (
          <Callout as="p" live={false} {...field.errorProps}>
            {error}
          </Callout>
        )}
        <Button variant="primary" type="submit" disabled={busy || pass.trim().length === 0}>
          {busy ? "Unlocking…" : "Unlock"}
        </Button>
      </form>

      {/* The folder itself is refusing access — retyping the passphrase can't fix that. */}
      {deniedPath !== null && (
        <RepairAccessButton path={deniedPath} onRepaired={onUnlocked} variant="secondary" />
      )}

      {status?.pointed_root && (
        <button
          className="text-xs text-ink4 underline underline-offset-2 hover:text-ink3"
          disabled={busy}
          onClick={() => setConfirmDetach(true)}
        >
          Can't unlock? Use a vault on this account instead
        </button>
      )}

      <DetachConfirm
        open={confirmDetach}
        onClose={() => setConfirmDetach(false)}
        status={status}
        onConfirm={() => void detach()}
      />

      {status?.location && (
        <p className="max-w-xs break-all text-xs text-faint">{status.location}</p>
      )}
    </div>
  );
}
