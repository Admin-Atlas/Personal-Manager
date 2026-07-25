// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The one-time density legacy-pin migration (PR: density axis). `standard` (WCAG 2.5.8) is the
// fresh-install default, but an existing install must be pinned to `compact` so the update disturbs
// nothing — and the user can Reset (or pick) their way to the compliant sizing. See initialDensity.

import { describe, expect, it } from "vitest";
import { initialDensity } from "./ThemeContext";

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
