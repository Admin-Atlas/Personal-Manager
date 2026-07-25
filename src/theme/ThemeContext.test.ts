// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The one-time legacy-pin migration shared by the two axes whose compliant default differs from
// today (density → `standard`/WCAG 2.5.8, contrast → `aa`/WCAG 1.4.3). A fresh install gets the
// compliant default; an existing install is pinned to the legacy value so the update disturbs
// nothing, and the user can Reset (or pick) their way up. See initialDensity / initialContrast.

import { describe, expect, it } from "vitest";
import { initialDensity, initialContrast } from "./ThemeContext";

describe("initialDensity — the density legacy pin", () => {
  it("gives a genuinely fresh install the compliant default (standard)", () => {
    expect(initialDensity(null, false)).toBe("standard");
  });

  it("pins an existing install (theme state present, no density stored) to legacy compact", () => {
    expect(initialDensity(null, true)).toBe("compact");
  });

  it("lets a stored density win over the migration, on either kind of install", () => {
    expect(initialDensity("comfortable", true)).toBe("comfortable");
    expect(initialDensity("compact", false)).toBe("compact");
    expect(initialDensity("standard", true)).toBe("standard");
  });

  it("falls back to the default for a corrupt stored value", () => {
    expect(initialDensity("enormous", true)).toBe("standard");
    expect(initialDensity("", false)).toBe("standard");
  });
});

describe("initialContrast — the contrast legacy pin", () => {
  it("gives a fresh install the compliant default (aa) and pins an existing install to legacy", () => {
    expect(initialContrast(null, false)).toBe("aa");
    expect(initialContrast(null, true)).toBe("legacy");
  });

  it("lets a stored value win, and falls back to the default when corrupt", () => {
    expect(initialContrast("high", true)).toBe("high");
    expect(initialContrast("legacy", false)).toBe("legacy");
    expect(initialContrast("ultra", true)).toBe("aa");
  });
});
