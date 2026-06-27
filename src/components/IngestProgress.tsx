// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Depth-aware progress. In the default "count" mode the document-engine file loop reports a known
// total (the `counted` IngestEvent), so at standard+ the bar fills and shows an "X of Y" count, and
// power adds a percentage. In "percent" mode the work has no countable items (e.g. the optional t-SNE
// download), so standard and power both show just the percentage. At minimal — or before the total is
// known (the opaque setup + model-download phase, where `total` is still null) — it falls back to the
// indeterminate shimmer. This is the one place the per-depth rule lives, so every progress surface
// (Documents ingest, Re-index modal, the t-SNE download, and any future one) renders consistently by
// passing `processed` / `total`.

import { useDepth } from "../theme";
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
}

export function IngestProgress({ processed, total, label, className, mode = "count" }: Props) {
  const { minimal, showMeta, showPower } = useDepth();
  const frac = total && total > 0 ? Math.min(1, processed / total) : null;
  // Minimal always shimmers; everyone shimmers until the total is known.
  const value = minimal ? null : frac;
  const showText = showMeta && frac !== null; // standard + power, once we have a total
  const pct = frac !== null ? Math.round(frac * 100) : 0;
  return (
    <div className={className}>
      <Progress value={value} label={label} />
      {showText && (
        <p className="mt-1 font-mono text-xs text-ink4">
          {mode === "percent" ? (
            `${pct}%`
          ) : (
            <>
              {processed} of {total}
              {showPower ? ` · ${pct}%` : ""}
            </>
          )}
        </p>
      )}
    </div>
  );
}
