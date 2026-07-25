// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// DOCUMENTED EXCEPTION to the "no hex literals" rule. GraphView colours project nodes
// categorically — it needs as many visually-distinct hues as there are projects, which the
// accent-driven token system (one accent + tinted neutrals) cannot express. This and the fixed
// modal-scrim tint are the only sanctioned non-token colours. Everything else around the graph
// (edges, halos, node strokes) still uses tokens.

import type { Mode } from "./profiles";

// The dark set is the original V1 GraphView palette; the light set is deepened for contrast on
// the near-white light-mode --bg. Indexed by (project index % length).
const DARK: readonly string[] = [
  "#60a5fa",
  "#34d399",
  "#f472b6",
  "#fbbf24",
  "#a78bfa",
  "#22d3ee",
  "#fb923c",
  "#4ade80",
  "#f87171",
  "#c084fc",
];
const LIGHT: readonly string[] = [
  "#2563eb",
  "#059669",
  "#db2777",
  "#d97706",
  "#7c3aed",
  "#0891b2",
  "#ea580c",
  "#16a34a",
  "#dc2626",
  "#9333ea",
];

// Colour-blind-safe (Okabe–Ito) categorical sets, used when the colour-blind axis is on. Mode-tuned
// at the two extremes the base palette can't share: yellow is invisible on the light --bg (→ a dark
// gold) and pure black is invisible on the dark --bg (→ a light neutral); the six chromatic hues
// carry unchanged. Eight distinct entries, mutually distinguishable under the common CVD types.
const CVD_DARK: readonly string[] = [
  "#56b4e9",
  "#e69f00",
  "#009e73",
  "#f0e442",
  "#0072b2",
  "#d55e00",
  "#cc79a7",
  "#bbbbbb",
];
const CVD_LIGHT: readonly string[] = [
  "#0072b2",
  "#e69f00",
  "#009e73",
  "#cc79a7",
  "#d55e00",
  "#56b4e9",
  "#8a6d00",
  "#000000",
];

export function graphColor(index: number, mode: Mode, colorblind = false): string {
  const palette = colorblind
    ? mode === "light"
      ? CVD_LIGHT
      : CVD_DARK
    : mode === "light"
      ? LIGHT
      : DARK;
  const i = ((index % palette.length) + palette.length) % palette.length;
  return palette[i];
}
