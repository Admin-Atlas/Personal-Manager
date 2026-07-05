// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The vault open-error gate (spec §2; B1-6): shown when the store *failed to open* at boot
// from a transient condition — antivirus or Windows Search momentarily holding the file, or a
// disk I/O blip — rather than being locked. It offers Retry (db::open's message is already
// friendly and retryable) so a passing hiccup no longer aborts the whole app. Distinct from
// VaultUnlock: there is nothing to type here — the key is fine, the file was just unavailable.

import { useState } from "react";
import { retryOpenVault } from "../lib/ipc";
import type { VaultStatus } from "../lib/types";
import { Button } from "./ui";

export function VaultOpenError({
  status,
  onResolved,
}: {
  status: VaultStatus;
  onResolved: () => void;
}) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(status.open_error);

  async function retry() {
    if (busy) return;
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

      <Button variant="primary" disabled={busy} onClick={() => void retry()}>
        {busy ? "Trying again…" : "Try again"}
      </Button>

      {status.location && (
        <p className="max-w-xs break-all text-xs text-faint">{status.location}</p>
      )}
    </div>
  );
}
