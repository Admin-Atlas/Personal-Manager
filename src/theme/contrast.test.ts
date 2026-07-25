// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The contrast-audit (the "do we adhere?" test, PR: contrast axis). Drives the real themeVars path
// for every System × Mode at each contrast level, converts the emitted oklch()/hex token values back
// to WCAG relative luminance, and asserts the level's targets: AA (1.4.3) lifts all body text to
// 4.5:1; High reaches AAA (7:1) for ink..ink3 and firms up ink4/faint (4.5:1) and the border edges
// (3:1, 1.4.11); Legacy is a no-op. Body text is checked against every background it sits on
// (bg/panel/surface) — the worst case. If a token ramp regresses, this fails in CI.

import { describe, expect, it } from "vitest";
import { themeVars, type ThemeVars } from "./tokens";
import { oklabLCH, oklchLuminance, contrastRatio } from "./oklab";
import { SYSTEMS, MODES, ACCENTS } from "./profiles";

// Luminance of a token value, whether it's an oklch(L C H) string or a #hex (the mono --bg pin).
function lumOf(value: string): number {
  if (value.startsWith("#")) {
    const { L, C, H } = oklabLCH(value);
    return oklchLuminance(L, C, H);
  }
  const m = value.match(/oklch\(([-\d.]+)\s+([-\d.]+)\s+([-\d.]+)\)/);
  if (!m) throw new Error(`un-parseable token value: ${value}`);
  return oklchLuminance(Number(m[1]), Number(m[2]), Number(m[3]));
}

const ratioOf = (v: ThemeVars, role: string, bg: string): number =>
  contrastRatio(lumOf(v[`--${role}`]), lumOf(v[`--${bg}`]));

const TEXT_BGS = ["bg", "panel", "surface"] as const;

describe("contrast axis — WCAG targets across every system × mode", () => {
  for (const system of SYSTEMS) {
    const accent = ACCENTS[system][0]; // each System's default accent (mono for slate)
    for (const mode of MODES) {
      it(`${system}/${mode}: AA lifts every body-text role to 4.5:1`, () => {
        const v = themeVars(system, mode, accent, false, "aa");
        for (const role of ["ink", "ink2", "ink3", "ink4"]) {
          for (const bg of TEXT_BGS) {
            expect(ratioOf(v, role, bg)).toBeGreaterThanOrEqual(4.5);
          }
        }
      });

      it(`${system}/${mode}: High reaches AAA body + firmer faint/borders`, () => {
        const v = themeVars(system, mode, accent, false, "high");
        for (const bg of TEXT_BGS) {
          for (const role of ["ink", "ink2", "ink3"]) {
            expect(ratioOf(v, role, bg)).toBeGreaterThanOrEqual(7);
          }
          for (const role of ["ink4", "faint"]) {
            expect(ratioOf(v, role, bg)).toBeGreaterThanOrEqual(4.5);
          }
        }
        // 1.4.11 non-text: the border edges vs the base background.
        for (const role of ["border", "border2"]) {
          expect(ratioOf(v, role, "bg")).toBeGreaterThanOrEqual(3);
        }
      });
    }
  }

  it("Legacy is a no-op (today's ramp) and AA genuinely changes ink4", () => {
    const legacy = themeVars("slate", "dark", "mono", false, "legacy");
    const untouched = themeVars("slate", "dark", "mono", false); // default param is legacy
    expect(legacy).toEqual(untouched);
    const aa = themeVars("slate", "dark", "mono", false, "aa");
    expect(aa["--ink4"]).not.toBe(legacy["--ink4"]);
  });
});
