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
} from "./profiles";

export type ThemeVars = Record<`--${string}`, string>;

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
      v["--accent"] = "#ffffff";
      v["--accent-text"] = "#ffffff";
      v["--accent-ink"] = EIGENGRAU; // dark text on the white accent fill
      v["--accent-soft"] = "rgba(255,255,255,0.12)";
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
): void {
  const vars = themeVars(system, mode, accent);
  for (const key of Object.keys(vars) as Array<keyof ThemeVars>) {
    el.style.setProperty(key, vars[key]);
  }
  el.dataset.system = system;
  el.dataset.mode = mode;
  el.dataset.depth = depth;
  el.style.colorScheme = mode; // native controls + scrollbars follow light/dark
}
