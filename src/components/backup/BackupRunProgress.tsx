// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Live progress for whichever backup or restore is in flight, plus the Stop button. One instance
// for the whole tab: the backend runs one op at a time and reports it on a single global channel.

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
}

export function BackupRunProgress({ running, phase, fraction, startedAt }: BackupRunProgressProps) {
  if (!(running || phase)) return null;
  return (
    <div className="mt-3">
      <IngestProgress
        processed={Math.round(fraction * 100)}
        // F-45: the upload/download fraction is a coarse per-destination fan-out step (0 then 1
        // for a single target), not real byte-progress — so shimmer (total=null) instead of a
        // bar frozen at 0% through a minutes-long transfer.
        total={isOpaquePhase(phase) ? null : 100}
        startedAt={startedAt ?? undefined}
        mode="percent"
        label={phase ? PHASE_LABEL[phase] : "Working"}
      />
      {running && (
        <Button variant="tertiary" className="mt-1" onClick={() => stopBackup()}>
          Stop
        </Button>
      )}
    </div>
  );
}
