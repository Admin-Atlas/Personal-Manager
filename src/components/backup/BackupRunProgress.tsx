// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Live progress for whichever backup or restore is in flight, plus the Stop button.
//
// Props-only, with no subscription of its own: the single mount snapshot + `backup://progress`
// subscription lives in `BackupSettings`, because the backend runs one op at a time and reports it
// on one global channel. So this may be RENDERED more than once — and needs to be. The tab's copy
// sits near the top, which on any normal window is roughly 800px above the "Back up now" button
// five sections down: pressing the button produced feedback that was off-screen, and the run read
// as though nothing had happened.
//
// A second instance therefore renders inside the destination panel that started the run, gated on
// `activeDest` — `BackupEvent` carries no destination and running/phase/fraction are global, so
// without that gate a Proton-only run would paint "Uploading" under Google Drive's button too.
// `showStop` keeps exactly one Stop button on the page: two would be ambiguous both to a user and
// to the section-scoped tests, which find controls by walking up from a section heading.

import type { BackupPhase } from "../../lib/types";
import { isOpaquePhase } from "../../lib/backup";
import { stopBackup } from "../../lib/ipc";
import { Button } from "../ui";
import { IngestProgress } from "../IngestProgress";

const PHASE_LABEL: Record<BackupPhase, string> = {
  snapshot: "Preparing a snapshot",
  pack: "Compressing & encrypting",
  upload: "Uploading",
  download: "Downloading",
  restore: "Decrypting & unpacking",
  validate: "Verifying",
};

export interface BackupRunProgressProps {
  running: boolean;
  phase: BackupPhase | null;
  fraction: number;
  startedAt: number | null;
  /** Render the Stop button. Exactly one instance on the page should — the tab-level one. */
  showStop?: boolean;
}

export function BackupRunProgress({
  running,
  phase,
  fraction,
  startedAt,
  showStop = true,
}: BackupRunProgressProps) {
  if (!(running || phase)) return null;
  return (
    <div className="mt-3">
      <IngestProgress
        processed={Math.round(fraction * 100)}
        // F-45: shimmer rather than a percentage for every phase whose fraction isn't a real
        // measure of the work — upload/download (a coarse per-destination fan-out step), and
        // snapshot/validate (which emit only 0 and 1 around one opaque call). `isOpaquePhase`
        // owns that list and documents why each phase is on it.
        total={isOpaquePhase(phase) ? null : 100}
        startedAt={startedAt ?? undefined}
        mode="percent"
        label={phase ? PHASE_LABEL[phase] : "Working"}
      />
      {running && showStop && (
        <Button variant="tertiary" className="mt-1" onClick={() => stopBackup()}>
          Stop
        </Button>
      )}
    </div>
  );
}
