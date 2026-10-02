// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { LocalRecommendations } from "../../lib/types";
import { formatGib } from "../../lib/format";
import { useDepth } from "../../theme";
import { sectionHelp, sectionLabel } from "./sections";
import { Button, Callout, SectionInfo, SectionLabel } from "../ui";

/**
 * "Your machine" — what PM read about this device, and the button that re-reads it.
 *
 * A pure view over the scan. Everything it shows was measured on this machine and none of it left
 * it, which is the claim the section's own copy makes and the reason this file makes no calls of
 * its own: the scan belongs to the tab, and this only renders it.
 */
export function LocalAiMachine({
  recs,
  loading,
  rescanning,
  onRescan,
  error,
}: {
  recs: LocalRecommendations | null;
  loading: boolean;
  rescanning: boolean;
  onRescan: () => void;
  /** Reading the hardware or sizing the list against it failed, said here rather than at the top
   *  of the tab. */
  error?: string | null;
}) {
  return (
    <div
      id="sec-localai-machine"
      data-settings-section
      data-help={sectionHelp("sec-localai-machine")}
      className="mt-5 border-t border-border pt-4"
    >
      <SectionLabel
        action={
          <Button variant="tertiary" size="sm" onClick={() => onRescan()} disabled={rescanning}>
            {rescanning ? "Scanning…" : "Re-scan"}
          </Button>
        }
      >
        {sectionLabel("sec-localai-machine")}
      </SectionLabel>
      {error && <Callout className="mt-2">{error}</Callout>}
      {loading ? (
        <p className="mt-2 text-xs text-ink4">Scanning your hardware…</p>
      ) : recs ? (
        <HardwareReadout recs={recs} />
      ) : (
        <p className="mt-2 text-xs text-ink4">Couldn't read your hardware.</p>
      )}
      <SectionInfo title="How PM reads your machine">
        <p>
          PM checks your memory, processor, and graphics card entirely on this device — nothing is
          sent anywhere. It uses this only to work out which local models would run well, and how
          fast.
        </p>
      </SectionInfo>
    </div>
  );
}

/** "22-07-2026" from the catalogue's `YYYY-MM-DD`, or the value as it came. */
function ddmmyyyy(date: string): string {
  const m = /^(\d{4})-(\d{2})-(\d{2})/.exec(date);
  return m ? `${m[3]}-${m[2]}-${m[1]}` : date;
}

function HardwareReadout({ recs }: { recs: LocalRecommendations }) {
  const { showPower } = useDepth();
  const h = recs.hardware;
  // The free memory every verdict in the list was scored against, read when it was sized — not the
  // cached scan's figure, which can be minutes older than the fits beside it.
  const free = recs.live_available_ram_gb ?? h.available_ram_gb;
  const rows: Array<[string, string]> = [
    ["Memory", `${formatGib(free)} free of ${formatGib(h.total_ram_gb)}`],
    [
      "Processor",
      `${h.cpu_brand ?? "—"}${h.cpu_cores ? ` · ${h.cpu_cores} cores` : ""}${h.cpu_threads ? ` / ${h.cpu_threads} threads` : ""}`,
    ],
    [
      "Graphics",
      h.gpu_name
        ? `${h.gpu_name}${h.vram_gb ? ` · ${formatGib(h.vram_gb)}${h.unified_memory ? " unified" : " VRAM"}` : ""}${h.gpu_bandwidth_gbps ? ` · ${h.gpu_bandwidth_gbps.toFixed(0)} GB/s published` : ""}`
        : "No dedicated GPU detected",
    ],
    ["Free disk", formatGib(h.disk_free_gb)],
  ];
  return (
    <div className="mt-3">
      <dl className="grid grid-cols-[auto_1fr] gap-x-4 gap-y-1 text-xs">
        {rows.map(([k, v]) => (
          <div key={k} className="contents">
            <dt className="text-ink4">{k}</dt>
            <dd className="text-ink2">{v}</dd>
          </div>
        ))}
      </dl>
      {h.is_wsl && (
        <p className="mt-1.5 text-xs text-ink4">
          Running under WSL — GPU access depends on your WSL setup.
        </p>
      )}
      {h.notes.length > 0 && <p className="mt-1.5 text-xs text-ink4">{h.notes.join(" ")}</p>}
      {/* The RAM reserve is subtracted on EVERY machine, so it is stated on every machine. This whole
          line used to be gated on having a discrete GPU, which meant a CPU-only box, an Apple
          Silicon Mac and any laptop on integrated graphics were never told their fit was scored
          against free RAM minus a reserve — the one number most likely to explain a verdict they
          disagreed with. Only the GPU half is conditional now. */}
      <p className="mt-1.5 text-xs text-ink4">
        Sized with ~{recs.reserve_gb.toFixed(0)} GB of RAM
        {h.vram_gb != null && !h.unified_memory
          ? ` and ~${recs.gpu_reserve_gb.toFixed(0)} GB of GPU memory`
          : ""}{" "}
        kept free, measured as PM scored these models.
      </p>
      {h.vram_gb != null && !h.unified_memory && h.gpu_bandwidth_gbps == null && (
        <p className="mt-1.5 text-xs text-ink4">
          Speed estimates use a default graphics-memory bandwidth — this card's exact model wasn't
          recognised.
        </p>
      )}
      {h.unified_memory && (
        <p className="mt-1.5 text-xs text-ink4">
          PM doesn't estimate speed on chips that share memory with the processor yet — how fast
          that memory is varies too much from chip to chip.
        </p>
      )}
      {showPower && (
        <p className="mt-1.5 text-xs text-ink4">
          PM's model list: version {recs.catalog_version}, built{" "}
          {ddmmyyyy(recs.catalog_generated_at)}.
        </p>
      )}
    </div>
  );
}
