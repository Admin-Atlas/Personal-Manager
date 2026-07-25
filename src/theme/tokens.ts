// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// themeVars(), ported from design-system-docs/DESIGN_TOKENS.md §6, plus the DOM glue that
// applies the computed custom properties to a root element. This is the single source of
// visual truth: every component reads only the resulting var(--…), never these tables.

import { oklabLCH, hexA } from "./oklab";
import {
  FONTS,
  RADII,
  PROFILES,
  STATUS,
  ROLES,
  STATUS_KEYS,
  MONO_ACCENT,
  MONO_RAMP,
  EIGENGRAU,
  type System,
  type Mode,
  type Depth,
  type Density,
} from "./profiles";

export type ThemeVars = Record<`--${string}`, string>;

/** The opt-in accessibility axes (Accessibility settings). Additive over the visual theme. All but
 *  `density` default to today's behaviour ({ fontScale: 1, reduceMotion: false, legibleFont: false });
 *  `density` defaults to `standard` for fresh installs but is pinned to `compact` (today's sizing)
 *  for existing installs by a one-time migration in ThemeContext — see {@link DENSITY_VARS}. */
export interface A11yTheme {
  /** Whole-UI text scale (1 = 100%). Multiplies the root font-size, so all rem sizing scales. */
  fontScale: number;
  /** Force motion off regardless of the OS `prefers-reduced-motion` setting. */
  reduceMotion: boolean;
  /** Swap the UI + heading faces for Atkinson Hyperlegible (a legible / dyslexia-friendly face). */
  legibleFont: boolean;
  /** Control density / touch-target size (WCAG 2.5.8 / 2.5.5). */
  density: Density;
}

// Atkinson Hyperlegible — the family name declared by @fontsource/atkinson-hyperlegible (imported in
// fonts.ts). Overrides --ui/--head only; --mono (numbers/code) is left untouched.
const LEGIBLE_STACK = '"Atkinson Hyperlegible", system-ui, sans-serif';

// Density → control-sizing custom properties, read by the ui/ primitives (never a blunt global
// `button{}` rule, which would swell calendar chips etc.). The visible switch track is separated
// from its tap target so `compact` keeps today's 20px LOOK while still flooring a ≥24px HIT area
// (WCAG 2.5.8 is satisfied by the actionable region, padding included). `standard` grows the visible
// track to 24px; `comfortable` reaches the 44px AAA target. `--tap-min` also floors Button / Select /
// SegmentedControl. Components fall back to the `standard` values via var()'s second arg, so the very
// first paint (before applyTheme runs) is already compliant.
export const DENSITY_VARS: Record<Density, Record<`--${string}`, string>> = {
  compact: {
    "--tap-min": "24px",
    "--tg-track-h": "20px",
    "--tg-track-w": "36px",
    "--tg-knob": "16px",
    // Knob rests at left:2px; on-travel of 14px lands it at 16px — exactly today's toggle geometry
    // (h-5 w-9 track, translate-x-0.5 → translate-x-4), so a pinned/legacy install shifts by 0px.
    "--tg-on": "14px",
  },
  standard: {
    "--tap-min": "24px",
    "--tg-track-h": "24px",
    "--tg-track-w": "44px",
    "--tg-knob": "20px",
    "--tg-on": "20px",
  },
  comfortable: {
    "--tap-min": "44px",
    "--tg-track-h": "28px",
    "--tg-track-w": "52px",
    "--tg-knob": "24px",
    "--tg-on": "24px",
  },
};

export function themeVars(system: System, mode: Mode, accent: string): ThemeVars {
  const stat = STATUS[system][mode];
  const v: ThemeVars = {};

  if (accent === MONO_ACCENT) {
    // Monochrome (Eigengrau) treatment: a chroma-0 neutral ramp with white text/accents (dark)
    // or near-black (light). No accent hue tints the cosmetics — only the *informational* feature
    // colours (the semantic status set just below, and the map palette in graphPalette.ts) stay
    // in colour. --bg is pinned to the exact Eigengrau hex in dark so the base colour is precise.
    const ramp = MONO_RAMP[mode];
    ROLES.forEach((r) => {
      v[`--${r}`] = `oklch(${ramp[r]} 0 0)`;
    });
    if (mode === "dark") {
      v["--bg"] = EIGENGRAU;
      // A soft off-white (~#F2F2F2), not pure white — sits a touch above --ink so accents still
      // read as "white" and pop, without the harsh glare/halation of #fff on the dark bg.
      v["--accent"] = "oklch(0.965 0 0)";
      v["--accent-text"] = "oklch(0.965 0 0)";
      v["--accent-ink"] = EIGENGRAU; // dark text on the white accent fill
      v["--accent-soft"] = "rgba(255,255,255,0.1)";
    } else {
      v["--accent"] = "oklch(0.2 0 0)";
      v["--accent-text"] = "oklch(0.2 0 0)";
      v["--accent-ink"] = "#ffffff";
      v["--accent-soft"] = "rgba(20,20,20,0.1)";
    }
  } else {
    const ramp = PROFILES[system][mode];
    const { C, H } = oklabLCH(accent);
    const ok = ([L, c]: readonly [number, number]): string => `oklch(${L} ${c} ${H})`;
    ROLES.forEach((r) => {
      v[`--${r}`] = ok(ramp[r]);
    });
    v["--accent"] = accent;
    v["--accent-text"] = mode === "light" ? `oklch(0.52 ${Math.min(C, 0.17)} ${H})` : accent;
    v["--accent-ink"] = `oklch(0.16 0.024 ${H})`;
    v["--accent-soft"] = hexA(accent, mode === "light" ? 0.14 : 0.15);
  }

  STATUS_KEYS.forEach((k, i) => {
    v[`--st-${k}`] = stat[i];
  });

  v["--head"] = FONTS[system].head;
  v["--ui"] = FONTS[system].ui;
  v["--mono"] = FONTS[system].mono;
  v["--radius"] = RADII[system][0];
  v["--radius-sm"] = RADII[system][1];

  return v;
}

// Apply the computed properties to a root element and stamp data-* hooks that drive per-system
// component branches and CSS selectors. Call with document.documentElement so var(--…) resolves
// app-wide, including portals/overlays mounted outside the React root.
export function applyTheme(
  el: HTMLElement,
  system: System,
  mode: Mode,
  accent: string,
  depth: Depth,
  a11y?: A11yTheme,
): void {
  const vars = themeVars(system, mode, accent);
  for (const key of Object.keys(vars) as Array<keyof ThemeVars>) {
    el.style.setProperty(key, vars[key]);
  }
  el.dataset.system = system;
  el.dataset.mode = mode;
  el.dataset.depth = depth;
  el.style.colorScheme = mode; // native controls + scrollbars follow light/dark

  // Accessibility axes — applied AFTER the token loop so the legible-font override wins over the
  // system faces themeVars just wrote. Re-derived on every call, so toggling an axis off restores
  // the theme default (the loop above already re-set --ui/--head from FONTS).
  if (a11y) {
    el.style.setProperty("--font-scale", String(a11y.fontScale));
    if (a11y.legibleFont) {
      el.style.setProperty("--ui", LEGIBLE_STACK);
      el.style.setProperty("--head", LEGIBLE_STACK);
    }
    if (a11y.reduceMotion) el.dataset.reducedMotion = "on";
    else delete el.dataset.reducedMotion;
    el.dataset.density = a11y.density;
    const dv = DENSITY_VARS[a11y.density];
    for (const key of Object.keys(dv) as Array<keyof typeof dv>) {
      el.style.setProperty(key, dv[key]);
    }
  }
}
