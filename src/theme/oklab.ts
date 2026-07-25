// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// sRGB hex -> OKLab, ported verbatim from design-system-docs/DESIGN_TOKENS.md §5. The matrix
// constants are load-bearing colour maths — do not refactor or "simplify" them.

export interface OkLCH {
  L: number;
  C: number;
  H: number; // hue in degrees, 0..360
}

export function oklabLCH(hex: string): OkLCH {
  const h = hex.replace("#", "");
  const lin = (v: string): number => {
    const n = parseInt(v, 16) / 255;
    return n <= 0.04045 ? n / 12.92 : Math.pow((n + 0.055) / 1.055, 2.4);
  };
  const r = lin(h.slice(0, 2)),
    g = lin(h.slice(2, 4)),
    b = lin(h.slice(4, 6));
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  const L = 0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s;
  const A = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
  const B = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
  let H = (Math.atan2(B, A) * 180) / Math.PI;
  if (H < 0) H += 360;
  return { L: +L.toFixed(3), C: +Math.hypot(A, B).toFixed(3), H: +H.toFixed(1) };
}

// rgba(...) from a hex + alpha — used for the --accent-soft tint. From DESIGN_TOKENS.md §6.
export function hexA(hex: string, a: number): string {
  const h = hex.replace("#", "");
  return `rgba(${parseInt(h.slice(0, 2), 16)},${parseInt(h.slice(2, 4), 16)},${parseInt(h.slice(4, 6), 16)},${a})`;
}

// WCAG relative luminance of an oklch(L C H) colour: the inverse of oklabLCH's forward transform
// (OKLab → linear-light sRGB, matrices from DESIGN_TOKENS.md §5) followed by the 0.2126/0.7152/0.0722
// weighting. Lets the contrast-audit test measure the token ramps as the browser would render them.
// Load-bearing colour maths — do not "simplify" the constants.
export function oklchLuminance(L: number, C: number, Hdeg: number): number {
  const a = C * Math.cos((Hdeg * Math.PI) / 180);
  const b = C * Math.sin((Hdeg * Math.PI) / 180);
  const l_ = L + 0.3963377774 * a + 0.2158037573 * b;
  const m_ = L - 0.1055613458 * a - 0.0638541728 * b;
  const s_ = L - 0.0894841775 * a - 1.291485548 * b;
  const l = l_ ** 3,
    m = m_ ** 3,
    s = s_ ** 3;
  const R = 4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s;
  const G = -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s;
  const B = -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s;
  const cl = (x: number): number => Math.max(0, Math.min(1, x));
  return 0.2126 * cl(R) + 0.7152 * cl(G) + 0.0722 * cl(B);
}

/** WCAG 2.x contrast ratio between two relative luminances (order-independent, 1–21). */
export function contrastRatio(y1: number, y2: number): number {
  return (Math.max(y1, y2) + 0.05) / (Math.min(y1, y2) + 0.05);
}
