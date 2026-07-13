// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Resolve a Mode *preference* (what the user picked in Settings) to a concrete light/dark Mode
// (what tokens/components actually render). This is the single place the four preferences collapse
// to two values, so everything downstream keeps seeing a plain `dark` | `light`:
//   • light / dark → themselves (explicit).
//   • system       → the OS light/dark setting (prefers-color-scheme).
//   • auto         → sunrise/sunset at the user's location (timezone-derived, or a manual override),
//                    computed offline by solar.ts. If no location is available, it degrades to the
//                    OS setting ("auto-fallback") rather than guessing.

import type { Mode, ModePref } from "./profiles";
import { isDaytime, nextTransition } from "./solar";
import { coordsFor, type Coords } from "./timezones";

/** How a resolved Mode was arrived at — surfaced in Settings so the choice is legible. */
export type ModeSource = "explicit" | "system" | "auto" | "auto-fallback";

export interface ModeResolution {
  /** The concrete Mode to render. */
  mode: Mode;
  source: ModeSource;
  /** The location used, when source is "auto" (for a Settings hint). */
  coords?: Coords;
  /** Instant of the next scheduled flip, when source is "auto" (for scheduling; may be absent on a
   *  polar day/night, where the caller re-checks on a daily poll instead). */
  nextChange?: Date;
}

/** True if the OS currently prefers dark. Defaults to dark if the query is unavailable. */
export function prefersDark(): boolean {
  try {
    return window.matchMedia("(prefers-color-scheme: dark)").matches;
  } catch {
    return true;
  }
}

/** Resolve `pref` at instant `now`. `override` is an optional user-entered "lat, lon" for auto. */
export function resolveMode(pref: ModePref, now: Date, override?: string | null): ModeResolution {
  if (pref === "light" || pref === "dark") return { mode: pref, source: "explicit" };
  if (pref === "system") return { mode: prefersDark() ? "dark" : "light", source: "system" };

  // auto — sunrise/sunset at the user's location (one shared derivation; see coordsFor).
  const coords = coordsFor(override);
  if (!coords) return { mode: prefersDark() ? "dark" : "light", source: "auto-fallback" };
  const day = isDaytime(now, coords[0], coords[1]);
  const next = nextTransition(now, coords[0], coords[1]);
  return {
    mode: day ? "light" : "dark",
    source: "auto",
    coords,
    ...(next ? { nextChange: next } : {}),
  };
}
