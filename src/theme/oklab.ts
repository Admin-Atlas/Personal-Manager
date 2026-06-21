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
  const r = lin(h.slice(0, 2)), g = lin(h.slice(2, 4)), b = lin(h.slice(4, 6));
  const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
  const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
  const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
  const L = 0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s;
  const A = 1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s;
  const B = 0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s;
  let H = (Math.atan2(B, A) * 180) / Math.PI;
  if (H < 0) H += 360;
  return { L: +L.toFixed(3), C: +Math.hypot(A, B).toFixed(3), H: +H.toFixed(1) };
}

// rgba(...) from a hex + alpha — used for the --accent-soft tint. From DESIGN_TOKENS.md §6.
export function hexA(hex: string, a: number): string {
  const h = hex.replace("#", "");
  return `rgba(${parseInt(h.slice(0, 2), 16)},${parseInt(h.slice(2, 4), 16)},${parseInt(h.slice(4, 6), 16)},${a})`;
}
