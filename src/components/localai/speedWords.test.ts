// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A speed figure is PM's estimate on every basis that has one, and no number at all on shared
// memory. What these pin is that the words never claim more than that: "about" wherever there is a
// figure, never "up to" or a "ceiling" (the estimate is no longer a best case), no "~" (the trust is
// said in words), no digits where there is no figure, and the mixture-of-experts caveat only where
// PM halves a figure — on a graphics card.

import { describe, expect, it } from "vitest";

import { CHANGELOG } from "../../lib/changelog";
import type { LocalChatSpeed, LocalSpeedBasis } from "../../lib/types";
import { howPmPicks } from "./pickWords";
import { replySecs, SPEED_LIST_NOTE, speedLong, speedShort } from "./speedWords";

const BASES: Array<LocalSpeedBasis | null> = [
  "gpu_published",
  "gpu_typical",
  "shared",
  "system",
  null,
];
const NUMERIC: LocalSpeedBasis[] = ["gpu_published", "gpu_typical", "system"];
const fit = (speed_basis: LocalSpeedBasis | null, est_tokens_per_sec: number | null = 71.4) => ({
  speed_basis,
  est_tokens_per_sec,
});
const HW = { gpu_bandwidth_gbps: 384 };
const SPEED: LocalChatSpeed = { floor_tps: 30, reply_tokens: 300, reply_secs: 10 };

/** Every string this file can make, on every basis, with and without the MoE caveat. */
function everything(): string[] {
  const out = [SPEED_LIST_NOTE];
  for (const basis of BASES)
    for (const moe of [false, true])
      out.push(speedShort(fit(basis)) ?? "", speedLong(fit(basis), HW, { moe }) ?? "");
  return out.filter(Boolean);
}

describe("speedShort", () => {
  it("says 'about' on every basis with a figure", () => {
    for (const basis of NUMERIC) expect(speedShort(fit(basis))).toBe("about 71 tok/s");
    expect(speedShort(fit("system", 22.2))).toBe("about 22 tok/s");
  });

  it("prints the whole number the chat floor is compared on", () => {
    // fit.rs `shown_tps`: the one-decimal figure, rounded — so 29.5 is "30", which clears a 30
    // floor, and 29.4 is "29", which doesn't.
    expect(speedShort(fit("gpu_published", 29.5))).toBe("about 30 tok/s");
    expect(speedShort(fit("gpu_published", 29.4))).toBe("about 29 tok/s");
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
  it("names the card's published speed, how close PM's tests came, and the power budget they ran on", () => {
    // The re-fit of 07-10-2026: ten builds of eight models, every figure within about a quarter (-10.9% to +23.0%)
    // and all but gemma 3 4b within 12%, on a card capped to a reduced power budget for most replies
    // — at full power it ran 1.19-1.38x faster, so the sentence says a card at full power is often
    // faster, and by how much on that one.
    const text = speedLong(fit("gpu_published"), HW);
    expect(text).toBe(
      "PM's estimate, not a measurement on this computer: your graphics card's published memory speed (384 GB/s), scaled by how fast ten builds of eight models really ran on one laptop graphics card PM tested — there, it came within about a quarter of each, and within about 12% of all but one. That laptop runs its card on a reduced power budget for most replies, so on a desktop card, or a laptop that keeps its card at full power, replies may well come faster than this: at full power, that laptop's own card was about 1.2 to 1.4 times as fast.",
    );
    // No bandwidth figure, no bracket.
    expect(speedLong(fit("gpu_published"), { gpu_bandwidth_gbps: null })).not.toMatch(/GB\/s/);
  });

  it("adds the mixture-of-experts caveat on both graphics-card bases, and nowhere else", () => {
    const MOE =
      / This model is a mixture of experts, which PM hasn't timed: going by one published report, PM halves its figure again, so take it with a pinch of salt\.$/;
    for (const basis of BASES) {
      const withMoe = speedLong(fit(basis), HW, { moe: true }) ?? "";
      if (basis === "gpu_published" || basis === "gpu_typical") expect(withMoe).toMatch(MOE);
      else expect(withMoe).not.toMatch(/mixture of experts/);
      expect(speedLong(fit(basis), HW) ?? "").not.toMatch(/mixture of experts/);
    }
  });

  it("says a typical speed is a guess either way", () => {
    expect(speedLong(fit("gpu_typical"), HW)).toBe(
      "PM doesn't recognise your graphics card, so this assumes a typical memory speed (400 GB/s), scaled by how fast ten builds of eight models really ran on one laptop graphics card PM tested. Your card's memory speed could be half that, or double.",
    );
    expect(speedLong(fit("system"), HW)).toMatch(/rough guide in either direction/);
  });

  it("names the scaling on an unrecognised card itself, since it is never shown beside the published one", () => {
    // A machine has one basis, so the pick card shows this sentence alone: "scaled the same way"
    // pointed at a sentence the reader never sees.
    for (const moe of [false, true]) {
      const text = speedLong(fit("gpu_typical"), HW, { moe }) ?? "";
      expect(text).not.toMatch(/the same way/);
      expect(text).toContain(
        "scaled by how fast ten builds of eight models really ran on one laptop graphics card PM tested",
      );
    }
  });

  it("puts no number on shared memory", () => {
    expect(speedLong(fit("shared"), HW)).not.toMatch(/\d/);
    expect(speedLong(fit("shared"), HW, { moe: true })).not.toMatch(/\d/);
  });

  it("says nothing when there is no figure", () => {
    expect(speedLong(fit(null), HW)).toBeNull();
  });
});

describe("the speed copy, everywhere", () => {
  it("never calls a figure a ceiling, says 'up to', or writes a tilde", () => {
    const strings = everything();
    expect(strings.length).toBeGreaterThan(8);
    for (const s of strings) {
      expect(s).not.toMatch(/up to/i);
      expect(s).not.toMatch(/\bceilings?\b/i);
      expect(s).not.toContain("~");
    }
  });

  it("never repeats what the run of 02-10-2026 claimed", () => {
    // Eight models, 20%, and a card held back by its power settings at the time: the 07-10-2026
    // re-fit replaced all three, on every basis and in the list's own line.
    for (const s of everything())
      expect(s).not.toMatch(/how fast eight models really ran|within 20%|held back/);
  });

  it("says the list's figures are estimates, not measurements on this computer", () => {
    expect(SPEED_LIST_NOTE).toMatch(
      /^Speeds are PM's estimates, not measurements on this computer:/,
    );
  });

  it("never says a card's figure comes from its published speed without allowing for a typical one", () => {
    // On a card PM doesn't recognise the figure rests on a typical 400 GB/s (`gpu_typical`), and copy
    // that can't see the basis is read on that machine too. Only `gpu_published`'s own sentence may
    // name the published speed alone: it is shown only where there is one.
    const release = CHANGELOG.find((e) => e.version === "3.138.0-alpha");
    const unscoped = [
      SPEED_LIST_NOTE,
      howPmPicks(SPEED),
      speedLong(fit("gpu_typical"), HW) ?? "",
      ...(release?.highlights ?? []),
    ];
    const claims = unscoped.flatMap(
      (s) => s.match(/[^.]*\bpublished\b[^.]*memory speed[^.]*\./g) ?? [],
    );
    // The note, the explainer and the What's New each name it: the test is not passing for want of a
    // sentence to check.
    expect(claims.length).toBeGreaterThanOrEqual(3);
    for (const c of claims) expect(c).toMatch(/\btypical\b/);
  });

  it("rests the mixture-of-experts halving on the one report it has, everywhere it is said", () => {
    // fit.rs `MOE_GPU_FACTOR` comes from a single published report (Qwen3.6 35B A3B on an RTX 4090),
    // and these sentences exist to say how far to trust the figure: "reports" claims more evidence
    // than PM has. The All models guide entry is pinned in LocalAiSettings.test.tsx.
    const release = CHANGELOG.find((e) => e.version === "3.138.0-alpha");
    const said = [
      speedLong(fit("gpu_published"), HW, { moe: true }) ?? "",
      ...(release?.highlights ?? []).filter((h) => /mixture-of-experts/.test(h)),
    ];
    expect(said).toHaveLength(2);
    for (const s of said) {
      expect(s).toMatch(/\bone published report\b/);
      expect(s).not.toMatch(/\breports\b/);
    }
  });
});

describe("replySecs", () => {
  it("says how long an answer of a few paragraphs takes, singular and plural", () => {
    expect(replySecs(42.7, SPEED)).toBe("7 seconds");
    expect(replySecs(71, SPEED)).toBe("4 seconds");
    expect(replySecs(300, SPEED)).toBe("1 second");
    // Never under one second, however fast.
    expect(replySecs(2000, SPEED)).toBe("1 second");
  });
});
