// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { eventColour, eventFill, eventHover, inkOn } from "./eventPalette";
import { oklabLCH, oklchLuminance, contrastRatio } from "./oklab";
import { ACCENTS, CONTRASTS, MODES, SYSTEMS } from "./profiles";
import { themeVars } from "./tokens";
import { sourcePalette } from "./sourcePalette";

const OWN = Array.from({ length: 11 }, (_, i) => eventColour(String(i + 1))!);

describe("Google's event colours", () => {
  it("names ids 1-11 as Google's picker does", () => {
    expect(OWN.map((c) => c.name)).toEqual([
      "Lavender",
      "Sage",
      "Grape",
      "Flamingo",
      "Banana",
      "Tangerine",
      "Peacock",
      "Graphite",
      "Blueberry",
      "Basil",
      "Tomato",
    ]);
  });

  it("has eleven distinct six-digit colours", () => {
    const hexes = OWN.map((c) => c.hex);
    expect(new Set(hexes).size).toBe(11);
    for (const h of hexes) expect(h).toMatch(/^#[0-9a-f]{6}$/);
  });

  it("gives no colour for none, or an id it doesn't know", () => {
    for (const id of [null, undefined, "", "0", "12", "constructor", "toString"]) {
      expect(eventColour(id)).toBeNull();
    }
  });
});

describe("eventFill and inkOn", () => {
  it("fill an event solid with its own colour, whatever the surface's calendar tint", () => {
    const lavender = eventColour("1");
    expect(eventFill(lavender, "#5b8cff", 18, "var(--surface)")).toBe("#7986cb");
    expect(eventFill(lavender, "#5b8cff", null)).toBe("#7986cb");
    expect(inkOn(lavender)).toEqual({ color: "#1f1f1f" });
    expect(inkOn(eventColour("11"))).toEqual({ color: "#ffffff" });
  });

  it("fall back to the calendar's tint and the token text, or to no fill where there's none", () => {
    expect(eventFill(null, "#5b8cff", 16)).toBe("color-mix(in oklab, #5b8cff 16%, transparent)");
    expect(eventFill(null, "#5b8cff", 18, "var(--surface)")).toBe(
      "color-mix(in oklab, #5b8cff 18%, var(--surface))",
    );
    expect(eventFill(null, "#5b8cff", null)).toBeUndefined();
    expect(inkOn(null)).toBeUndefined();
  });
});

const hexLum = (hex: string): number => {
  const { L, C, H } = oklabLCH(hex);
  return oklchLuminance(L, C, H);
};

describe("text on an event's own colour", () => {
  // Title, time and place are all drawn in the colour's ink, at 9-11px: the 4.5:1 floor. The fill is
  // solid, so this holds in every look and mode alike. And each ink is the better of dark and white.
  it("reads at 4.5:1 or better, on every colour", () => {
    for (const own of OWN) {
      const ratio = contrastRatio(hexLum(own.ink), hexLum(own.hex));
      expect(ratio, own.name).toBeGreaterThanOrEqual(4.5);
      const other = own.ink === "#ffffff" ? "#1f1f1f" : "#ffffff";
      expect(ratio, own.name).toBeGreaterThan(contrastRatio(hexLum(other), hexLum(own.hex)));
    }
  });

  // `brightness(f)` scales every gamma-encoded channel of the element, text included, and clamps at
  // the top: brightening Basil under white text took it to 4.27:1. Each colour's hover keeps 4.5:1.
  it("still reads while the event is hovered", () => {
    const channels = (hex: string) => [1, 3, 5].map((i) => parseInt(hex.slice(i, i + 2), 16) / 255);
    const hovered = (hex: string, f: number) => channels(hex).map((c) => Math.min(1, c * f));
    expect(eventHover(null)).toBe("hover:brightness-110");
    for (const own of OWN) {
      const f = eventHover(own) === "hover:brightness-90" ? 0.9 : 1.1;
      const ratio = contrastRatio(lumOfSrgb(hovered(own.ink, f)), lumOfSrgb(hovered(own.hex, f)));
      expect(ratio, own.name).toBeGreaterThanOrEqual(4.5);
    }
  });
});

// Measuring a calendar's tint as the browser paints it. `color-mix(in oklab, X p%, base)` is linear in
// OKLab; a mix into `transparent` is X at alpha p, which the browser then composites over what's
// behind it in gamma-encoded sRGB.
type Lab = readonly [number, number, number];

function labOf(value: string): Lab {
  let L: number, C: number, H: number;
  if (value.startsWith("#")) {
    ({ L, C, H } = oklabLCH(value));
  } else {
    const m = value.match(/oklch\(([-\d.]+)\s+([-\d.]+)\s+([-\d.]+)\)/);
    if (!m) throw new Error(`un-parseable colour: ${value}`);
    [L, C, H] = [Number(m[1]), Number(m[2]), Number(m[3])];
  }
  const h = (H * Math.PI) / 180;
  return [L, C * Math.cos(h), C * Math.sin(h)];
}

const lumOfLab = ([L, a, b]: Lab): number =>
  oklchLuminance(L, Math.hypot(a, b), (Math.atan2(b, a) * 180) / Math.PI);

// OKLab -> gamma-encoded sRGB (the inverse transform oklchLuminance uses), for alpha compositing.
function srgbOf([L, a, b]: Lab): [number, number, number] {
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
  const lin = [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
  ].map((x) => Math.max(0, Math.min(1, x)));
  const enc = (x: number) => (x <= 0.0031308 ? 12.92 * x : 1.055 * x ** (1 / 2.4) - 0.055);
  return [enc(lin[0]), enc(lin[1]), enc(lin[2])];
}

function lumOfSrgb(rgb: readonly number[]): number {
  const dec = (x: number) => (x <= 0.04045 ? x / 12.92 : ((x + 0.055) / 1.055) ** 2.4);
  return 0.2126 * dec(rgb[0]) + 0.7152 * dec(rgb[1]) + 0.0722 * dec(rgb[2]);
}

/** Luminance of `fill` at `p` over `under`: mixed in OKLab when opaque, composited when not. */
function fillLum(fill: Lab, p: number, under: Lab, opaque: boolean): number {
  const at = (i: number) => fill[i] * p + under[i] * (1 - p);
  if (opaque) return lumOfLab([at(0), at(1), at(2)]);
  const [f, u] = [srgbOf(fill), srgbOf(under)];
  return lumOfSrgb([0, 1, 2].map((i) => f[i] * p + u[i] * (1 - p)));
}

describe("text on a calendar's tint", () => {
  // An event without its own colour sits on its calendar's tint, and its time is small mono text. It
  // was --ink4, which fell to 3.5:1 on some tints (and --ink3 no better); --ink2 clears 4.5:1 on all
  // of them: every calendar hue (the colour-blind set too), every look, mode, contrast and accent.
  const SITES: { name: string; pct: number; opaque: boolean; unders: `--${string}`[] }[] = [
    { name: "a Day/Week card", pct: 18, opaque: true, unders: ["--surface"] },
    { name: "a Month chip", pct: 16, opaque: false, unders: ["--bg", "--panel"] },
    { name: "a Month bar", pct: 20, opaque: false, unders: ["--bg", "--panel"] },
    { name: "an all-day bar", pct: 22, opaque: false, unders: ["--bg", "--panel"] },
    { name: "a Terminal bar", pct: 14, opaque: false, unders: ["--bg", "--panel"] },
  ];
  for (const site of SITES) {
    it(`keeps the time readable on ${site.name}`, () => {
      for (const system of SYSTEMS) {
        for (const mode of MODES) {
          for (const contrast of CONTRASTS) {
            for (const accent of ACCENTS[system]) {
              for (const colorblind of [false, true]) {
                const v = themeVars(system, mode, accent, colorblind, contrast);
                for (const hue of sourcePalette(system, accent, colorblind)) {
                  for (const under of site.unders) {
                    const y = fillLum(labOf(hue), site.pct / 100, labOf(v[under]), site.opaque);
                    const ratio = contrastRatio(lumOfLab(labOf(v["--ink2"])), y);
                    expect(
                      ratio,
                      `${hue} on ${under}, ${system}/${mode}/${contrast}/${accent}`,
                    ).toBeGreaterThanOrEqual(4.5);
                  }
                }
              }
            }
          }
        }
      }
    });
  }
});
