// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A speed figure is a ceiling on a recognised card, a rough guess elsewhere, and no number at all on
// shared memory. What these pin is that the words never claim more than that: no "~" (which reads as
// "roughly this"), "up to" only where PM has a ceiling to stand behind, and no digits where it has
// none.

import { describe, expect, it } from "vitest";

import type { LocalSpeedBasis } from "../../lib/types";
import { SPEED_LIST_NOTE, speedLong, speedShort } from "./speedWords";

const BASES: Array<LocalSpeedBasis | null> = [
  "gpu_published",
  "gpu_typical",
  "shared",
  "system",
  null,
];
const fit = (speed_basis: LocalSpeedBasis | null, est_tokens_per_sec: number | null = 71.4) => ({
  speed_basis,
  est_tokens_per_sec,
});
const HW = { gpu_bandwidth_gbps: 384 };

describe("speedShort", () => {
  it("never writes a tilde", () => {
    for (const basis of BASES) expect(speedShort(fit(basis)) ?? "").not.toContain("~");
  });

  it("says 'up to' only for a card PM recognises", () => {
    expect(speedShort(fit("gpu_published"))).toBe("up to 71 tok/s");
    expect(speedShort(fit("gpu_typical"))).toBe("about 71 tok/s");
    expect(speedShort(fit("system", 22.2))).toBe("about 22 tok/s");
    for (const basis of BASES.filter((b) => b !== "gpu_published")) {
      expect(speedShort(fit(basis)) ?? "").not.toMatch(/up to/);
    }
  });

  it("puts no number on shared memory", () => {
    expect(speedShort(fit("shared"))).toBe("speed not estimated");
    expect(speedShort(fit("shared"))).not.toMatch(/\d/);
  });

  it("shows nothing when there is no figure", () => {
    expect(speedShort(fit(null))).toBeNull();
    expect(speedShort(fit("gpu_published", null))).toBeNull();
  });
});

describe("speedLong", () => {
  it("never writes a tilde, on any basis", () => {
    for (const basis of BASES) {
      expect(speedLong(fit(basis), HW, { moe: true }) ?? "").not.toContain("~");
    }
    expect(SPEED_LIST_NOTE).not.toContain("~");
  });

  it("names the card's published speed and PM's own measurement for a recognised card", () => {
    const text = speedLong(fit("gpu_published"), HW);
    expect(text).toContain("(384 GB/s)");
    expect(text).toContain("10–60% slower");
    expect(text).not.toMatch(/mixture-of-experts/);
  });

  it("adds the mixture-of-experts caveat where PM claims a ceiling", () => {
    expect(speedLong(fit("gpu_published"), HW, { moe: true })).toMatch(
      / For a mixture-of-experts model the real figure can be much lower\.$/,
    );
  });

  it("says a typical speed is a guess either way", () => {
    expect(speedLong(fit("gpu_typical"), HW)).toMatch(/half that, or double/);
    expect(speedLong(fit("system"), HW)).toMatch(/rough guide in either direction/);
  });

  it("puts no number on shared memory", () => {
    expect(speedLong(fit("shared"), HW)).not.toMatch(/\d/);
  });

  it("says nothing when there is no figure", () => {
    expect(speedLong(fit(null), HW)).toBeNull();
  });
});
