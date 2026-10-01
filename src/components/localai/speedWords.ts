// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// How a speed estimate is worded, wherever it is shown — from what it was worked out from.
//
// The figure is an upper bound, not a forecast (fit.rs, `tokens_per_sec`): memory bandwidth divided
// by the bytes read per token. On a discrete graphics card PM recognises it has measured +11% to +55%
// above real decode speed, so it is a ceiling and says "up to". On a card PM doesn't recognise, or
// from system RAM, it rests on a typical bandwidth and can be wrong either way, so it says "about".
// On memory shared with the processor PM puts no number on it at all. No "~" anywhere: a tilde reads
// as "roughly this", which is the one thing a ceiling is not.

import type { LocalFitResult, LocalHardware } from "../../lib/types";

/** The line under a list of models, never folded: what every speed figure in it is. */
export const SPEED_LIST_NOTE =
  "Speeds are estimates from published or typical memory speeds, not measurements — “up to” figures are ceilings, and real replies are slower. “What do these numbers mean?” explains each one.";

type SpeedFit = Pick<LocalFitResult, "est_tokens_per_sec" | "speed_basis">;

/** "up to 71 tok/s" | "about 22 tok/s" | "speed not estimated", or null when there is no figure. */
export function speedShort(fit: SpeedFit): string | null {
  const n = fit.est_tokens_per_sec;
  switch (fit.speed_basis) {
    case "gpu_published":
      return n == null ? null : `up to ${n.toFixed(0)} tok/s`;
    case "gpu_typical":
    case "system":
      return n == null ? null : `about ${n.toFixed(0)} tok/s`;
    case "shared":
      return "speed not estimated";
    default:
      return null;
  }
}

/**
 * Where the figure comes from and how far to trust it, as a sentence — or null when there is no
 * figure. Shown beside a figure, never folded: it is the half of the number that makes it honest.
 *
 * `moe`: the model reads fewer parameters per token than it holds, so the bandwidth arithmetic is at
 * its most optimistic — said only where PM claims a ceiling at all.
 */
export function speedLong(
  fit: SpeedFit,
  hw: Pick<LocalHardware, "gpu_bandwidth_gbps"> | null,
  { moe = false }: { moe?: boolean } = {},
): string | null {
  switch (fit.speed_basis) {
    case "gpu_published": {
      const bw = hw?.gpu_bandwidth_gbps;
      const text = `Worked out from your graphics card's published memory speed${
        bw != null ? ` (${Math.round(bw)} GB/s)` : ""
      }, not measured on this computer, so it's a best case: in PM's own checks on one laptop graphics card, real replies came 10–35% slower. Take it with a pinch of salt.`;
      return moe
        ? `${text} For a mixture-of-experts model the real figure can be much lower.`
        : text;
    }
    case "gpu_typical":
      return "PM doesn't recognise your graphics card, so this uses a typical memory speed (400 GB/s). Yours could be half that, or double.";
    case "system":
      return "From a typical memory speed (40 GB/s), not measured on this computer — a rough guide in either direction.";
    case "shared":
      return "PM doesn't estimate speed on chips that share memory with the processor yet — how fast that memory is varies too much from chip to chip.";
    default:
      return null;
  }
}
