// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { ReactNode } from "react";

import type { LocalFitResult, LocalFitVerdict } from "../../lib/types";
import { formatGib } from "../../lib/format";

/**
 * How a fit VERDICT is shown, wherever it is shown.
 *
 * Shared by the catalog cards and the on-disk cards because they are answering the same question
 * about the same arithmetic, and two spellings of "comfortable" would be two claims about a
 * machine rather than one.
 */
const VERDICT: Record<LocalFitVerdict, { label: string; token: string }> = {
  comfortable: { label: "Comfortable", token: "--st-quick" },
  tight: { label: "Tight fit", token: "--st-look" },
  halved_context: { label: "Reduced context", token: "--st-look" },
  stay_on_cloud: { label: "Too big — stay on cloud", token: "--st-due" },
  unknown: { label: "Unknown", token: "--ink4" },
};

export function FitBadge({ verdict }: { verdict: LocalFitVerdict }) {
  const v = VERDICT[verdict];
  return (
    <span
      className="rounded-[var(--radius-sm)] px-1.5 py-0.5 text-[0.625rem] font-medium"
      style={{
        color: `var(${v.token})`,
        background: `color-mix(in oklab, var(${v.token}) 15%, transparent)`,
      }}
    >
      {v.label}
    </span>
  );
}

/** The per-config mono metric spans (quant · context · speed · memory), shared by the single- and
 *  two-config (Split) card layouts. */
function ConfigMetrics({ fit }: { fit: LocalFitResult }) {
  return (
    <>
      {fit.quant && <span>{fit.quant}</span>}
      {fit.context != null && <span>{(fit.context / 1024).toFixed(0)}k ctx</span>}
      {fit.kv === "q8_0" && <span>q8_0 KV</span>}
      {fit.est_tokens_per_sec != null && <span>~{fit.est_tokens_per_sec.toFixed(0)} tok/s</span>}
      {fit.est_memory_gb != null && <span>{formatGib(fit.est_memory_gb)}</span>}
    </>
  );
}

/** One labelled config row in a Split card: the mono metrics (with a “q8_0 KV” chip when the cache was
 *  compressed) plus this config's situational caveat (system-RAM vs GPU, halved/tight). */
export function ConfigRow({
  label,
  fit,
  action,
}: {
  label: string;
  fit: LocalFitResult;
  /** This rung's own way to get it — a Download, an Installed chip, or nothing. Rendered on the
   *  label line so the action sits beside the config it fetches, never a card-level button the user
   *  has to guess the meaning of. */
  action?: ReactNode;
}) {
  const caveat = fit.notes.join(" ");
  return (
    <div>
      <div className="flex flex-wrap items-baseline gap-x-2 gap-y-0.5">
        <span className="shrink-0 text-[0.625rem] font-medium text-ink3">{label}</span>
        <div className="flex flex-wrap gap-x-3 gap-y-0.5 font-mono text-[0.6875rem] text-ink4">
          <ConfigMetrics fit={fit} />
        </div>
        {action && <div className="ml-auto shrink-0">{action}</div>}
      </div>
      {caveat && <p className="mt-0.5 text-[0.625rem] text-ink4">{caveat}</p>}
    </div>
  );
}
