// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// themeVars(), ported from design-system-docs/DESIGN_TOKENS.md §6, plus the DOM glue that
// applies the computed custom properties to a root element. This is the single source of
// visual truth: every component reads only the resulting var(--…), never these tables.

import { oklabLCH, hexA } from "./oklab";
import {
  FONTS, RADII, PROFILES, STATUS, ROLES, STATUS_KEYS,
  type System, type Mode, type Depth,
} from "./profiles";

export type ThemeVars = Record<`--${string}`, string>;

export function themeVars(system: System, mode: Mode, accent: string): ThemeVars {
  const ramp = PROFILES[system][mode];
  const stat = STATUS[system][mode];
  const { C, H } = oklabLCH(accent);
  const ok = ([L, c]: readonly [number, number]): string => `oklch(${L} ${c} ${H})`;
  const v: ThemeVars = {};

  ROLES.forEach((r) => { v[`--${r}`] = ok(ramp[r]); });
  STATUS_KEYS.forEach((k, i) => { v[`--st-${k}`] = stat[i]; });

  v["--head"] = FONTS[system].head;
  v["--ui"] = FONTS[system].ui;
  v["--mono"] = FONTS[system].mono;
  v["--radius"] = RADII[system][0];
  v["--radius-sm"] = RADII[system][1];

  v["--accent"] = accent;
  v["--accent-text"] = mode === "light" ? `oklch(0.52 ${Math.min(C, 0.17)} ${H})` : accent;
  v["--accent-ink"] = `oklch(0.16 0.024 ${H})`;
  v["--accent-soft"] = hexA(accent, mode === "light" ? 0.14 : 0.15);

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
