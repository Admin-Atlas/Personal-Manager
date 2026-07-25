// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// How density and contrast resolve a stored value. Both axes briefly offered a below-baseline level
// (`compact` / `legacy`) carrying PM's original sizing and ramps, pinned onto existing installs so
// the accessibility epic wouldn't change their look under them. Those levels are withdrawn, and
// dropping them from DENSITIES/CONTRASTS *is* the migration: a stored one is no longer in the
// allow-list, so it falls back to the compliant default. These tests pin exactly that — a stored
// `compact`/`legacy` must land on `standard`/`aa`, never survive as a value the picker can't show.

import { describe, expect, it } from "vitest";
import { storedDensity, storedContrast } from "./ThemeContext";
import { CONTRASTS, DENSITIES } from "./profiles";

describe("storedDensity", () => {
  it("defaults an install with nothing stored to the compliant standard", () => {
    expect(storedDensity(null)).toBe("standard");
  });

  it("migrates a stored `compact` (the withdrawn level) up to standard", () => {
    expect(storedDensity("compact")).toBe("standard");
  });

  it("keeps a still-offered stored value", () => {
    expect(storedDensity("comfortable")).toBe("comfortable");
    expect(storedDensity("standard")).toBe("standard");
  });

  it("falls back to the default for a corrupt stored value", () => {
    expect(storedDensity("enormous")).toBe("standard");
    expect(storedDensity("")).toBe("standard");
  });
});

describe("storedContrast", () => {
  it("defaults an install with nothing stored to the compliant AA", () => {
    expect(storedContrast(null)).toBe("aa");
  });

  it("migrates a stored `legacy` (the withdrawn level) up to AA", () => {
    expect(storedContrast("legacy")).toBe("aa");
  });

  it("keeps a still-offered stored value, and falls back when corrupt", () => {
    expect(storedContrast("high")).toBe("high");
    expect(storedContrast("aa")).toBe("aa");
    expect(storedContrast("ultra")).toBe("aa");
  });
});

describe("the withdrawn levels are really gone", () => {
  it("no longer lists compact / legacy, so nothing can select them", () => {
    expect(DENSITIES).not.toContain("compact");
    expect(CONTRASTS).not.toContain("legacy");
    // Both axes still offer a real choice — this isn't a collapse to one value.
    expect(DENSITIES.length).toBeGreaterThan(1);
    expect(CONTRASTS.length).toBeGreaterThan(1);
  });
});
