// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Per-device VIEW prefs for the memory Map, shared by the Map header controls (GraphView) and the
// Settings → Map section — both read and write the same localStorage keys, so the defaults and the
// cohesion clamp live here once. (The node cap / t-SNE enablement are vault-travelling `map` prefs
// the backend reads instead — see SettingsView.)

import { clamp } from "./math";

/** The Map's arrangement: by-project force layout, or semantic proximity. */
export type MapLayoutMode = "project" | "semantic";

export const MAP_MODE_KEY = "pm.map.layoutMode";
export const MAP_COHESION_KEY = "pm.map.cohesion";

/** The stored arrangement; anything but "semantic" (including absent) is the by-project default. */
export function readMapMode(): MapLayoutMode {
  return localStorage.getItem(MAP_MODE_KEY) === "semantic" ? "semantic" : "project";
}

/** Project-cohesion weight (0 = pure meaning, the default), clamped to the 0..0.5 the UI offers. */
export function readMapCohesion(): number {
  const raw = Number(localStorage.getItem(MAP_COHESION_KEY));
  return Number.isFinite(raw) ? clamp(raw, 0, 0.5) : 0;
}
