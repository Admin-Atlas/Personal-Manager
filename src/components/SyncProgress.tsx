// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { Button, ConfirmDialog } from "./ui";
import { IngestProgress } from "./IngestProgress";

/**
 * The **in-flight sync UI** shared by every index-only connector: the progress bar, the "keeps running
 * in the background" reassurance, and the confirm-gated Stop control (stopping keeps everything indexed
 * so far). `label` is the bar caption ("Indexing your Drive", "Indexing folder", …); `sizeQuestion` is
 * the stop-box opener (the cloud connectors warn about *size*, local folders just ask). The Stop confirm
 * dialog lives here so all three connectors share one copy.
 */
export function SyncProgress({
  processed,
  total,
  label,
  startedAt,
  sizeQuestion = "Changed your mind about the size of this?",
  stopping,
  confirmStop,
  setConfirmStop,
  onStop,
}: {
  processed: number;
  total: number | null;
  label: string;
  /** Epoch ms the backend started this sync, so the Power-depth elapsed timer survives a remount. */
  startedAt?: number | null;
  sizeQuestion?: string;
  stopping: boolean;
  confirmStop: boolean;
  setConfirmStop: (v: boolean) => void;
  onStop: () => void;
}) {
  return (
    <>
      <div className="mt-3">
        <IngestProgress
          processed={processed}
          total={total}
          label={label}
          startedAt={startedAt ?? undefined}
        />
        <p className="mt-1 text-xs text-ink4">
          Indexing keeps running in the background — you can leave this page and come back later;
          we’ll keep working.
        </p>
        <div
          className="mt-2 rounded-[var(--radius)] px-3 py-2 text-xs text-ink3"
          style={{ background: "color-mix(in oklab, var(--st-due) 12%, transparent)" }}
        >
          {sizeQuestion} <span className="text-ink2">Stopping keeps everything indexed so far</span>{" "}
          — those files stay searchable; the rest just won’t be indexed until you sync again.
          <div className="mt-2">
            <Button
              variant="tertiary"
              onClick={() => setConfirmStop(true)}
              disabled={stopping}
              className="px-2 py-1 text-xs hover:text-st-due disabled:opacity-40"
            >
              {stopping ? "Stopping…" : "Stop indexing"}
            </Button>
          </div>
        </div>
      </div>

      <ConfirmDialog
        open={confirmStop}
        title="Stop indexing?"
        danger
        confirmLabel="Stop indexing"
        onConfirm={() => {
          setConfirmStop(false);
          onStop();
        }}
        onClose={() => setConfirmStop(false)}
      >
        Everything indexed so far is kept and stays searchable — only the files not yet reached will
        be left out, and a later sync picks them up where this one stopped. You can resume any time
        with “Sync now”.
      </ConfirmDialog>
    </>
  );
}
