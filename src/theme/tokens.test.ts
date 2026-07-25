// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The accessibility-axis half of applyTheme (Accessibility settings): --font-scale, the legible-font
// override of --ui/--head, the data-reduced-motion stamp, the density control vars, and the
// colour-blind status swap. Verifies the defaults are inert and that toggling an axis is re-derived
// cleanly, since the whole theme is re-applied on every change.

import { describe, expect, it } from "vitest";
import { applyTheme, themeVars, type A11yTheme } from "./tokens";

// A base config at every axis default; spread overrides per test, so adding an axis doesn't churn
// every call site here.
const BASE: A11yTheme = {
  fontScale: 1,
  reduceMotion: false,
  legibleFont: false,
  density: "standard",
  colorblind: false,
};

describe("applyTheme accessibility axes", () => {
  it("stamps the font scale and leaves the defaults inert", () => {
    const el = document.createElement("div");
    applyTheme(el, "slate", "dark", "mono", "standard", BASE);
    expect(el.style.getPropertyValue("--font-scale")).toBe("1");
    expect(el.dataset.reducedMotion).toBeUndefined();
    expect(el.style.getPropertyValue("--ui")).not.toContain("Atkinson");
    expect(el.dataset.density).toBe("standard");
  });

  it("applies a larger scale, forced reduced motion, and the legible font when set", () => {
    const el = document.createElement("div");
    applyTheme(el, "slate", "dark", "mono", "standard", {
      ...BASE,
      fontScale: 1.3,
      reduceMotion: true,
      legibleFont: true,
    });
    expect(el.style.getPropertyValue("--font-scale")).toBe("1.3");
    expect(el.dataset.reducedMotion).toBe("on");
    expect(el.style.getPropertyValue("--ui")).toContain("Atkinson Hyperlegible");
    expect(el.style.getPropertyValue("--head")).toContain("Atkinson Hyperlegible");
  });

  it("clears the reduced-motion stamp when it's toggled back off", () => {
    const el = document.createElement("div");
    applyTheme(el, "slate", "dark", "mono", "standard", { ...BASE, reduceMotion: true });
    expect(el.dataset.reducedMotion).toBe("on");
    applyTheme(el, "slate", "dark", "mono", "standard", BASE);
    expect(el.dataset.reducedMotion).toBeUndefined();
  });

  it("stamps the density vars + data-density and re-derives when the level changes", () => {
    const el = document.createElement("div");
    applyTheme(el, "slate", "dark", "mono", "standard", { ...BASE, density: "compact" });
    expect(el.dataset.density).toBe("compact");
    expect(el.style.getPropertyValue("--tap-min")).toBe("24px");
    // compact keeps today's 20px track (an existing install's look is undisturbed)
    expect(el.style.getPropertyValue("--tg-track-h")).toBe("20px");

    applyTheme(el, "slate", "dark", "mono", "standard", { ...BASE, density: "comfortable" });
    expect(el.dataset.density).toBe("comfortable");
    expect(el.style.getPropertyValue("--tap-min")).toBe("44px");
    expect(el.style.getPropertyValue("--tg-track-h")).toBe("28px");
  });

  it("swaps the semantic status colours for the CVD set when colour-blind is on", () => {
    const base = themeVars("slate", "dark", "mono", false);
    const cvd = themeVars("slate", "dark", "mono", true);
    // The status vars differ under the axis...
    expect(cvd["--st-due"]).not.toBe(base["--st-due"]);
    // ...while the neutral ramp is untouched (only status is swapped).
    expect(cvd["--bg"]).toBe(base["--bg"]);
  });
});
