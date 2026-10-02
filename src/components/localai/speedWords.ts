// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// How a speed estimate is worded, wherever it is shown — from what it was worked out from.
//
// The figure is an estimate, not a measurement on this computer (fit.rs, `tokens_per_sec`): memory
// bandwidth divided by the bytes the model file says each token reads. On a discrete graphics card
// that is scaled by how fast eight models really ran on one laptop card PM tested, where it came
// within 20% of each — a card held back by its power settings at the time, so a healthy one likely
// runs faster. From system RAM it rests on a typical bandwidth and can be wrong either way. Every
// figure says "about"; on memory shared with the processor PM puts no number on it at all. No "~"
// anywhere: the copy says how far to trust the figure in words, beside it.

import type { LocalChatSpeed, LocalFitResult, LocalHardware } from "../../lib/types";

/** The line under a list of models, never folded: what every speed figure in it is. */
export const SPEED_LIST_NOTE =
  "Speeds are PM's estimates, not measurements on this computer: from published or typical memory speeds, and on a graphics card scaled by how fast models really ran on one laptop graphics card PM tested. “What do these numbers mean?” explains each one.";

type SpeedFit = Pick<LocalFitResult, "est_tokens_per_sec" | "speed_basis">;

/** "about 71 tok/s" | "speed not estimated", or null when there is no figure. The whole number is
 *  the one the chat floor is compared on (fit.rs `shown_tps`), so a card never says "about 30" beside
 *  "under 30". */
export function speedShort(fit: SpeedFit): string | null {
  const n = fit.est_tokens_per_sec;
  switch (fit.speed_basis) {
    case "gpu_published":
    case "gpu_typical":
    case "system":
      return n == null ? null : `about ${n.toFixed(0)} tok/s`;
    case "shared":
      return "speed not estimated";
    default:
      return null;
  }
}

/** Said after either graphics-card sentence for a mixture-of-experts model: the card's figure for
 *  one is halved (fit.rs `MOE_GPU_FACTOR`), going by one published report rather than PM's own
 *  timing. */
const MOE_SUFFIX =
  " This model is a mixture of experts, which PM hasn't timed: going by one published report, PM halves its figure again, so take it with a pinch of salt.";

/**
 * Where the figure comes from and how far to trust it, as a sentence — or null when there is no
 * figure. Shown beside a figure, never folded: it is the half of the number that makes it honest.
 *
 * `moe`: the model reads fewer parameters per token than it holds, and on a graphics card PM halves
 * its figure on one published report, not a model it has timed — said on both card bases, and never
 * from system memory, where no such factor applies.
 */
export function speedLong(
  fit: SpeedFit,
  hw: Pick<LocalHardware, "gpu_bandwidth_gbps"> | null,
  { moe = false }: { moe?: boolean } = {},
): string | null {
  const suffix = moe ? MOE_SUFFIX : "";
  switch (fit.speed_basis) {
    case "gpu_published": {
      const bw = hw?.gpu_bandwidth_gbps;
      return `PM's estimate, not a measurement on this computer: your graphics card's published memory speed${
        bw != null ? ` (${Math.round(bw)} GB/s)` : ""
      }, scaled by how fast eight models really ran on one laptop graphics card PM tested — there, it came within 20% of each. That card was held back by its power settings at the time, so yours may well be faster.${suffix}`;
    }
    case "gpu_typical":
      // Said on its own (a machine has one basis), so it names the scaling rather than pointing at
      // the published-speed sentence for it.
      return `PM doesn't recognise your graphics card, so this assumes a typical memory speed (400 GB/s), scaled by how fast eight models really ran on one laptop graphics card PM tested. Your card's memory speed could be half that, or double.${suffix}`;
    case "system":
      return "From a typical memory speed (40 GB/s), not measured on this computer — a rough guide in either direction.";
    case "shared":
      return "PM doesn't estimate speed on chips that share memory with the processor yet — how fast that memory is varies too much from chip to chip.";
    default:
      return null;
  }
}

/** How long an answer of a few paragraphs (`speed.reply_tokens`) takes to stream at `tps`: "1 second"
 *  or "N seconds", never under one. */
export function replySecs(tps: number, speed: LocalChatSpeed): string {
  const n = Math.max(1, Math.round(speed.reply_tokens / tps));
  return n === 1 ? "1 second" : `${n} seconds`;
}
