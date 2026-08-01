// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Shared restore result — surfaced at the top of the tab so either the file OR the Proton/Google
// restore flow shows it prominently, not buried under whichever section started it.
//
// Rendered unconditionally and short-circuiting on `restored == null` INSIDE the component, rather
// than being wrapped in `{restored && …}` by its parent: `restoreAsPrivate` is a user choice that
// survives `restored` going back to null today, and mounting the component with the card would
// reset it.

import { useState } from "react";

import type { RestoreSummary } from "../../lib/types";
import { formatDateTime } from "../../lib/format";
import { switchToVault } from "../../lib/ipc";
import { Button } from "../ui";

export interface RestoredVaultCardProps {
  restored: RestoreSummary | null;
  setError: (m: string | null) => void;
}

export function RestoredVaultCard({ restored, setError }: RestoredVaultCardProps) {
  const [switching, setSwitching] = useState(false);
  // When a restored vault was passphrase-protected ("shareable"), the user chooses on this machine:
  // make it private (default — none of the sharing setup travels in a backup), or keep the passphrase.
  const [restoreAsPrivate, setRestoreAsPrivate] = useState(true);

  async function doSwitch() {
    if (!restored) return;
    setError(null);
    try {
      setSwitching(true);
      // A device-mode restore is already private; only a passphrase restore honours the choice.
      const makePrivate = restored.key_mode === "passphrase" ? restoreAsPrivate : true;
      await switchToVault(restored.target_dir, makePrivate);
      // The active vault changed underneath the whole app; reload so every view re-reads it.
      window.location.reload();
    } catch (e) {
      setError(String(e));
      setSwitching(false);
    }
  }

  if (!restored) return null;

  return (
    <div className="mt-3 max-w-sm rounded-[var(--radius-sm)] border border-border2 bg-surface p-3">
      <p className="text-sm text-ink2">Restored a vault, ready to use.</p>
      <p className="mt-1 text-xs text-ink4">
        From a backup made {formatDateTime(restored.created_at)}. It&rsquo;s in a new folder; your
        current vault is still active until you switch.
      </p>
      {restored.key_mode === "passphrase" && (
        <fieldset className="mt-2 flex flex-col gap-1" disabled={switching}>
          <legend className="text-xs text-ink4">
            This backup was passphrase-protected for sharing. On this device:
          </legend>
          <label className="flex cursor-pointer items-start gap-2 text-xs text-ink3">
            <input
              type="radio"
              name="restore-privacy"
              className="mt-0.5"
              checked={restoreAsPrivate}
              onChange={() => setRestoreAsPrivate(true)}
            />
            <span>
              <span className="text-ink2">Make it private to this device</span> — recommended; your
              notes are re-encrypted with a device key and open without a passphrase.
            </span>
          </label>
          <label className="flex cursor-pointer items-start gap-2 text-xs text-ink3">
            <input
              type="radio"
              name="restore-privacy"
              className="mt-0.5"
              checked={!restoreAsPrivate}
              onChange={() => setRestoreAsPrivate(false)}
            />
            <span>
              <span className="text-ink2">Keep it passphrase-protected</span> — notes stay encrypted
              at rest; you can share the vault again later.
            </span>
          </label>
        </fieldset>
      )}
      <div className="mt-2">
        <Button variant="primary" onClick={doSwitch} disabled={switching}>
          {switching ? "Switching…" : "Switch to the restored vault"}
        </Button>
      </div>
    </div>
  );
}
