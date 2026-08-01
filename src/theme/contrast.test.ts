// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The contrast-audit (the "do we adhere?" test, PR: contrast axis). Drives the real themeVars path
// for every System × Mode at each contrast level, converts the emitted oklch()/hex token values back
// to WCAG relative luminance, and asserts the level's targets: AA (1.4.3) lifts all body text to
// 4.5:1; High reaches AAA (7:1) for ink..ink3 and firms up ink4/faint (4.5:1) and the border edges
// (3:1, 1.4.11). Body text is checked against every background it sits on (bg/panel/surface) — the
// worst case. If a token ramp regresses, this fails in CI.
//
// The neutral ramp was the whole audit for a while, and that was the hole: --st-* is text too, and
// because it sits outside the contrast axis nothing was watching it. 15 of 18 STATUS.light cells and
// 4 of 6 STATUS_CVD.light cells were below AA — `look` as low as 3.06:1 — at every setting the app
// offered. The status case below closes that, and it sweeps every accent rather than the default,
// because the accent hue tints the backgrounds the status colours are calibrated against.

import { describe, expect, it } from "vitest";
import { boost, themeVars, type ThemeVars } from "./tokens";
import { oklabLCH, oklchLuminance, contrastRatio } from "./oklab";
import { SYSTEMS, MODES, ACCENTS, STATUS_KEYS } from "./profiles";

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

      // The semantic status colours are TEXT: the error banner's message, Field's role="alert", the
      // "Due soon" chip, every connector failure string. They render at text-xs, so the 3:1
      // large-text exemption never applies. They also sit OUTSIDE the contrast axis — boost() only
      // touches the neutral ramp and themeVars emits --st-* verbatim, so `aa` and `high` are
      // byte-identical here and High contrast rescues nothing. Both levels are asserted anyway, so
      // that the day someone routes --st-* through boost(), this test is already watching.
      //
      // Every accent, not just the default: the accent hue tints bg/panel/surface, and the light
      // table is calibrated on the WORST of them. `mono` stays in the loop — it is not a colour, but
      // it is a real theme state producing real (chroma-0) backgrounds, and it happens to be the
      // worst case for slate. Both palettes, because the colour-blind axis swaps the whole row.
      for (const contrast of ["aa", "high"] as const) {
        for (const colorblind of [false, true]) {
          const name = `${system}/${mode}/${contrast}${colorblind ? "/cvd" : ""}`;
          it(`${name}: every --st-* clears 4.5:1 as text, on every background`, () => {
            for (const anyAccent of ACCENTS[system]) {
              const v = themeVars(system, mode, anyAccent, colorblind, contrast);
              for (const key of STATUS_KEYS) {
                for (const bg of TEXT_BGS) {
                  expect(ratioOf(v, `st-${key}`, bg)).toBeGreaterThanOrEqual(4.5);
                }
              }
            }
          });
        }
      }
    }
  }

  it("defaults to AA, and High lifts strictly more of the ramp than AA does", () => {
    const implicit = themeVars("slate", "dark", "mono", false);
    const aa = themeVars("slate", "dark", "mono", false, "aa");
    // The default param is the compliant baseline — no caller can land below AA by omitting it.
    expect(implicit).toEqual(aa);

    // AA moves ONLY the lowest text tier; High also firms up ink3, faint and the borders. Pinning
    // both halves keeps "AA is the light touch, High is the firm one" true rather than assumed.
    // (ink4 is deliberately NOT in this list — both levels lift it by the same calibrated amount,
    // because 4.5:1 is where that tier needs to land either way.)
    const high = themeVars("slate", "dark", "mono", false, "high");
    for (const role of ["ink3", "faint", "border", "border2"]) {
      expect(high[`--${role}`]).not.toBe(aa[`--${role}`]);
    }
    // …and AA leaves those roles exactly where the System's ramp put them (boost is identity there).
    const lc: readonly [number, number] = [0.62, 0.01];
    expect(boost(lc, "ink3", "dark", "aa")).toEqual([...lc]);
    expect(boost(lc, "ink4", "dark", "aa")).not.toEqual([...lc]);
  });
});
