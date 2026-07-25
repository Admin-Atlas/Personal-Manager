// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The accessibility-axis half of applyTheme (Accessibility settings, PR3): --font-scale, the
// legible-font override of --ui/--head, and the data-reduced-motion stamp. Verifies the defaults are
// inert (nothing changes at fontScale 1 / no reduce / no legible font) and that toggling an axis off
// is re-derived cleanly, since the whole theme is re-applied on every change.

import { describe, expect, it } from "vitest";
import { applyTheme } from "./tokens";

describe("applyTheme accessibility axes", () => {
  it("stamps the font scale and leaves the defaults inert", () => {
    const el = document.createElement("div");
    applyTheme(el, "slate", "dark", "mono", "standard", {
      fontScale: 1,
      reduceMotion: false,
      legibleFont: false,
    });
    expect(el.style.getPropertyValue("--font-scale")).toBe("1");
    expect(el.dataset.reducedMotion).toBeUndefined();
    expect(el.style.getPropertyValue("--ui")).not.toContain("Atkinson");
  });

  it("applies a larger scale, forced reduced motion, and the legible font when set", () => {
    const el = document.createElement("div");
    applyTheme(el, "slate", "dark", "mono", "standard", {
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
    const on = { fontScale: 1, reduceMotion: true, legibleFont: false };
    const off = { fontScale: 1, reduceMotion: false, legibleFont: false };
    applyTheme(el, "slate", "dark", "mono", "standard", on);
    expect(el.dataset.reducedMotion).toBe("on");
    applyTheme(el, "slate", "dark", "mono", "standard", off);
    expect(el.dataset.reducedMotion).toBeUndefined();
  });
});
