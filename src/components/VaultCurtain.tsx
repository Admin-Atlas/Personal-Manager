// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The vault curtain (spec §5): shown when another OS profile is the active writer of a
// shared vault, so this instance's store is closed and it must not write. "Continue here"
// cooperatively asks the other profile to hand over (the backend takes the baton once it
// releases, then lifts this curtain via the acquired event). A crashed holder (stale
// heartbeat) can be force-taken, but only behind the spec's explicit warning.

import { useState } from "react";
import { continueHere, forceTakeVault } from "../lib/ipc";
import type { VaultLockStatus } from "../lib/types";
import { Button, Callout } from "./ui";

export function VaultCurtain({
  status,
  reason,
  onChange,
}: {
  status: VaultLockStatus;
  reason: "other-active" | "handed-off";
  onChange: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [requested, setRequested] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function run(action: () => Promise<void>, thenRequested: boolean) {
    setBusy(true);
    setError(null);
    try {
      await action();
      if (thenRequested) setRequested(true);
      onChange(); // re-query status; the acquired event lifts the curtain on success
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  const where = status.other_profile ? `“${status.other_profile}”` : "another profile";

  return (
    <div className="flex h-full flex-col items-center justify-center gap-4 bg-bg px-6 text-center">
      <div className="flex h-12 w-12 items-center justify-center rounded-full border border-border text-ink2">
        {/* Two-windows glyph — no icon dependency. */}
        <svg width="22" height="22" viewBox="0 0 24 24" fill="none" aria-hidden="true">
          <rect x="3" y="5" width="13" height="10" rx="2" stroke="currentColor" strokeWidth="1.6" />
          <path
            d="M8 19h10a2 2 0 0 0 2-2V9"
            stroke="currentColor"
            strokeWidth="1.6"
            strokeLinecap="round"
          />
        </svg>
      </div>

      <div>
        <h1 className="font-ui text-lg font-semibold text-ink">
          {reason === "handed-off"
            ? "Now active in another profile"
            : "PM is open in another profile"}
        </h1>
        <p className="mt-1 max-w-sm text-sm text-ink4">
          PM is the active writer in {where}. Your vault is safe — only one profile writes at a
          time. Continue here to take over.
        </p>
      </div>

      {status.stale ? (
        <div className="flex max-w-sm flex-col items-center gap-2">
          <Callout as="p">The other instance may not have saved its last change.</Callout>
          <Button variant="primary" disabled={busy} onClick={() => run(forceTakeVault, false)}>
            {busy ? "Taking over…" : "Take over here"}
          </Button>
        </div>
      ) : requested ? (
        <p className="text-sm text-ink4">Waiting for the other profile to hand over…</p>
      ) : (
        <Button variant="primary" disabled={busy} onClick={() => run(continueHere, true)}>
          {busy ? "Working…" : "Continue here"}
        </Button>
      )}

      {error && <p className="max-w-sm break-all text-xs text-st-due">{error}</p>}
    </div>
  );
}
