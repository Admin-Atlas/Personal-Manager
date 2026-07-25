// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The colour-blind axis's categorical-palette swaps (PR: colour-blind-safe palettes). graphColor and
// sourcePalette/sourceColors return their normal, System/accent-derived hues by default and the
// Okabe–Ito CVD set when the axis is on. The semantic status swap is covered in tokens.test.ts.

import { describe, expect, it } from "vitest";
import { graphColor } from "./graphPalette";
import { sourceColors, sourcePalette, sourceShapeIndex } from "./sourcePalette";

describe("colour-blind categorical palettes", () => {
  it("graphColor returns the default palette normally and the CVD set when on", () => {
    expect(graphColor(0, "dark")).toBe("#60a5fa"); // unchanged default
    expect(graphColor(0, "dark", true)).toBe("#56b4e9"); // Okabe–Ito sky blue
    expect(graphColor(0, "light", true)).toBe("#0072b2"); // light-mode-tuned
    // still wraps by modulo over the CVD set
    expect(graphColor(8, "dark", true)).toBe(graphColor(0, "dark", true));
  });

  it("sourcePalette swaps to the CVD set independent of System/accent", () => {
    const normal = sourcePalette("slate", "#5b8cff");
    const cvd = sourcePalette("slate", "#5b8cff", true);
    expect(cvd).not.toEqual(normal);
    expect(cvd[0]).toBe("#56b4e9");
    // System/accent no longer influence the set under the axis
    expect(sourcePalette("terminal", "#9ece6a", true)).toEqual(cvd);
  });

  it("sourceColors assigns distinct CVD hues, stable across a re-sort", () => {
    const map = sourceColors(["b", "a", "c"], "slate", "mono", true);
    expect(new Set(map.values()).size).toBe(3);
    // assignment walks the sorted unique ids, so 'a' takes slot 0
    expect(map.get("a")).toBe("#56b4e9");
    expect(sourceColors(["c", "a", "b"], "slate", "mono", true)).toEqual(map);
  });

  it("sourceShapeIndex tracks the same sorted slots as sourceColors", () => {
    const shapes = sourceShapeIndex(["b", "a", "c"]);
    expect(shapes.get("a")).toBe(0);
    expect(shapes.get("b")).toBe(1);
    expect(shapes.get("c")).toBe(2);
    // pure function of the id SET — a re-sort of the same ids yields the same slots (so a source's
    // shape tracks its colour, which uses the same assignment)
    expect(sourceShapeIndex(["c", "a", "b"])).toEqual(shapes);
    // an unknown id (e.g. an overlay pseudo-calendar) has no slot → the dot stays a plain circle
    expect(shapes.get("milestones")).toBeUndefined();
  });
});
