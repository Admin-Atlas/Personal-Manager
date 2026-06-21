// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Depth = feature reveal, NOT a layout change. Components gate optional content on these
// predicates (e.g. `{showMeta && <MetaLine/>}`); they must never fork the layout per depth.
// See design-system-docs/README.md "Depth (feature reveal — not a layout change)".

import { useTheme } from "./ThemeContext";
import type { Depth } from "./profiles";

const RANK: Record<Depth, number> = { min: 0, standard: 1, power: 2 };

export interface DepthState {
  depth: Depth;
  atLeast: (d: Depth) => boolean;
  minimal: boolean;   // depth === "min": hide meta, larger type, more air
  showMeta: boolean;  // depth >= standard: meta lines, model footers, secondary columns
  showPower: boolean; // depth === "power": cost, token counts, timestamps, keybind hints
}

export function useDepth(): DepthState {
  const { depth } = useTheme();
  return {
    depth,
    atLeast: (d) => RANK[depth] >= RANK[d],
    minimal: depth === "min",
    showMeta: RANK[depth] >= RANK.standard,
    showPower: depth === "power",
  };
}
