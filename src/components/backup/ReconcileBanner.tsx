// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The banner shown at the top of a connected panel when the destination holds more of THIS vault's
// archives than keep-last-N — offering to keep them all (raise N) or trim to N now.
//
// One component for both destinations: the three actions it fires are already keyed by destination
// at the panel root, which is where they have to live (raising N is a SCHEDULE write, and trimming
// touches whichever destination's busy flag feeds the shared `busy` gate).

import { Button } from "../ui";

export interface ReconcileBannerProps {
  /** This vault's own archive count at the destination; null until the prefix and listing load. */
  present: number | null;
  over: boolean;
  dismissed: boolean;
  /** Outcome of "Delete oldest", reported IN the banner rather than in the panel-wide sink. */
  note: string | null;
  keepN: number | null;
  busy: boolean;
  /** This destination's own busy flag, so its Trimming… label is not lit by the other one. */
  destBusy: boolean;
  onRaiseKeepN: () => void;
  onPruneOldest: () => void;
  onDismiss: () => void;
}

export function ReconcileBanner({
  present,
  over,
  dismissed,
  note,
  keepN,
  busy,
  destBusy,
  onRaiseKeepN,
  onPruneOldest,
  onDismiss,
}: ReconcileBannerProps) {
  if (!over || dismissed || present == null || keepN == null) {
    // A successful trim clears `over`, which would take the banner — and its outcome line — away
    // in the same render. Keep the note on screen after the banner it belongs to has gone.
    return note ? <p className="text-sm text-ink3">{note}</p> : null;
  }
  return (
    <div
      className="flex flex-col gap-2 rounded-[var(--radius)] border px-3 py-2.5 text-sm text-ink3"
      style={{
        borderColor: "color-mix(in oklab, var(--st-due) 35%, transparent)",
        background: "color-mix(in oklab, var(--st-due) 12%, transparent)",
      }}
    >
      <p>
        This destination holds <span className="font-medium">{present}</span> backups of this vault
        — more than your keep-last-{keepN} limit. Older backups aren&rsquo;t trimmed automatically
        until you reconcile this.
      </p>
      <div className="flex flex-wrap gap-2">
        <Button variant="secondary" onClick={onRaiseKeepN} disabled={busy}>
          Keep all {present}
        </Button>
        <Button variant="tertiary" onClick={onPruneOldest} disabled={busy || destBusy}>
          {destBusy ? "Trimming…" : `Delete oldest, keep ${keepN}`}
        </Button>
        <Button variant="tertiary" onClick={onDismiss} disabled={busy}>
          Dismiss
        </Button>
      </div>
      {note && <p className="text-sm">{note}</p>}
    </div>
  );
}
