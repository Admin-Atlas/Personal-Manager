// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Depth-aware ingest/rebuild progress. The document-engine file loop reports a known
// total (the `counted` IngestEvent), so at standard+ the bar fills and shows an "X of Y"
// count, and power adds a percentage. At minimal — or before the total is known (the
// opaque setup + model-download phase, where `total` is still null) — it falls back to the
// indeterminate shimmer. This is the one place the per-depth rule lives, so every progress
// surface (Documents ingest, Re-index modal, and any future one) renders consistently by
// passing `processed` / `total`.

import { useDepth } from "../theme";
import { Progress } from "./ui";

interface Props {
  /** Files finished so far (done + skipped + failed). */
  processed: number;
  /** Total files this run will work through, or null until the `counted` event arrives. */
  total: number | null;
  /** Accessible label for the bar (e.g. "Ingesting documents", "Re-indexing"). */
  label: string;
  className?: string;
}

export function IngestProgress({ processed, total, label, className }: Props) {
  const { minimal, showMeta, showPower } = useDepth();
  const frac = total && total > 0 ? Math.min(1, processed / total) : null;
  // Minimal always shimmers; everyone shimmers until the total is known.
  const value = minimal ? null : frac;
  const showCount = showMeta && frac !== null; // standard + power, once we have a total
  const pct = frac !== null ? Math.round(frac * 100) : 0;
  return (
    <div className={className}>
      <Progress value={value} label={label} />
      {showCount && (
        <p className="mt-1 font-mono text-xs text-ink4">
          {processed} of {total}
          {showPower ? ` · ${pct}%` : ""}
        </p>
      )}
    </div>
  );
}
