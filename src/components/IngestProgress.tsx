// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Depth-aware progress. In the default "count" mode the document-engine file loop reports a known
// total (the `counted` IngestEvent), so at standard+ the bar fills and shows an "X of Y" count, and
// power adds a percentage. In "percent" mode the work has no countable items (e.g. the optional t-SNE
// download), so standard and power both show just the percentage. At minimal — or before the total is
// known (the opaque setup + model-download phase, where `total` is still null) — it falls back to the
// indeterminate shimmer. At Power an elapsed timer is shown on the right, INCLUDING during the shimmer
// phase (the longest ops run with no total yet, so that's exactly when it helps). This is the one place
// the per-depth rule lives, so every progress surface (Documents ingest, Re-index modal, the t-SNE
// download, and any future one) renders consistently by passing `processed` / `total`.

import { useRef } from "react";
import { useDepth } from "../theme";
import { useNowTick } from "../lib/useNowTick";
import { formatElapsed } from "../lib/format";
import { Progress } from "./ui";

interface Props {
  /** Items finished so far (count mode: done + skipped + failed; percent mode: pass 0–100). */
  processed: number;
  /** Total items this run will work through, or null until known (count mode: the `counted` event;
   *  percent mode: pass 100). */
  total: number | null;
  /** Accessible label for the bar (e.g. "Ingesting documents", "Re-indexing", "Downloading t-SNE"). */
  label: string;
  className?: string;
  /** "count" (default) → "X of Y" (+ % at power). "percent" → just the percentage (no item count). */
  mode?: "count" | "percent";
  /** Epoch ms the operation actually started, for an exact Power-depth elapsed timer. Every detached
   *  job now carries one in its backend snapshot (`started_at_ms` on the sync / rebuild / backup
   *  states), so a bar reopened mid-run counts from the true start rather than from when it
   *  reappeared. Omit it and the timer falls back to the mount instant — correct only for a bar whose
   *  mount IS the start (the Rebuild modal, which fires the rebuild itself) or for the on-demand
   *  component downloads, which have no backend snapshot to restore from at all. */
  startedAt?: number;
}

export function IngestProgress({
  processed,
  total,
  label,
  className,
  mode = "count",
  startedAt,
}: Props) {
  const { minimal, showMeta, showPower } = useDepth();
  const frac = total && total > 0 ? Math.min(1, processed / total) : null;
  // Minimal always shimmers; everyone shimmers until the total is known.
  const value = minimal ? null : frac;
  const showText = showMeta && frac !== null; // standard + power, once we have a total
  const pct = frac !== null ? Math.round(frac * 100) : 0;

  // The Power-depth elapsed timer. Ticks each second at power (a slow 60s tick otherwise, so it's
  // ~free when hidden). `startedAt` when the caller knows the true start, else the mount instant.
  const mountedAt = useRef(Date.now());
  const now = useNowTick(showPower ? 1000 : 60_000);
  const elapsed = showPower
    ? formatElapsed(now.getTime() - (startedAt ?? mountedAt.current))
    : null;

  // Spoken value for screen readers: the count (or percent) rather than a bare "%", once known.
  const valueText =
    frac === null ? undefined : mode === "percent" ? `${pct}%` : `${processed} of ${total}`;

  return (
    <div className={className}>
      <Progress value={value} label={label} valueText={valueText} />
      {(showText || elapsed) && (
        // Rendered whenever there's a count line OR an elapsed timer — so at Power the timer is visible
        // even during the shimmer phase, where the count line (which needs a total) is not.
        <div className="mt-1 flex items-center justify-between gap-2 font-mono text-xs text-ink4">
          <span>
            {showText
              ? mode === "percent"
                ? `${pct}%`
                : `${processed} of ${total}${showPower ? ` · ${pct}%` : ""}`
              : ""}
          </span>
          {elapsed && <span>{elapsed}</span>}
        </div>
      )}
    </div>
  );
}
