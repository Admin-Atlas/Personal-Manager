// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The only state the design layer owns: { system, modePref, accent, depth } (+ an optional manual
// location for the sunrise/sunset mode). Persisted two ways: localStorage stays the fast-path so
// the very first paint is already themed (a synchronous read, no IPC round-trip → no flash), and
// it is *also* mirrored into the encrypted settings table via `set_pref` so the theme travels with
// the data folder when it's backed up or moved to another machine. On a fresh machine (localStorage
// empty at boot) we hydrate from the stored blob; on the same machine localStorage already holds
// the values, so nothing flashes and the store is simply refreshed.
//
// Mode has a preference/resolved split: the user picks `light | dark | system | auto` (what we
// persist and show in Settings), and resolveMode.ts collapses that to the concrete `light | dark`
// that tokens and every component actually read. `system` follows the OS; `auto` follows sunrise/
// sunset at the user's (timezone-derived or manually set) location — recomputed here on a timer and
// whenever the app regains focus, so the app quietly flips itself day↔night.

import { createContext, useContext, useEffect, useState, type ReactNode } from "react";
import { applyTheme } from "./tokens";
import {
  ACCENTS,
  SYSTEMS,
  MODE_PREFS,
  DEPTHS,
  DENSITIES,
  CONTRASTS,
  type System,
  type Mode,
  type ModePref,
  type Depth,
  type Density,
  type Contrast,
} from "./profiles";
import { resolveMode, type ModeResolution, type ModeSource } from "./resolveMode";
import type { Coords } from "./timezones";
import { getPref, setPref } from "../lib/ipc";

// The settings-table key under which the full theme blob is mirrored (see below).
const PREF_KEY = "appearance";

export interface ThemeState {
  system: System;
  /** The resolved Mode that tokens/components render (never `system`/`auto`). */
  mode: Mode;
  /** What the user picked for Mode — persisted and shown in Settings. */
  modePref: ModePref;
  /** How `mode` was resolved from `modePref` (for the Settings hint). */
  modeSource: ModeSource;
  /** The location driving the `auto` (sunrise/sunset) mode, when one is available. */
  modeCoords?: Coords;
  /** Instant of the next scheduled day↔night flip under `auto`, if known. */
  modeNextChange?: Date;
  accent: string;
  depth: Depth;
  /** Optional user-entered "lat, lon" override for the `auto` mode ("" = use the device timezone). */
  autoLocation: string;
  setSystem: (s: System) => void;
  setModePref: (m: ModePref) => void;
  setAccent: (a: string) => void;
  setDepth: (d: Depth) => void;
  setAutoLocation: (v: string) => void;
  /** Whether the **learning tools** — the Review and Teach tabs — are shown. A Depth-keyed reveal
   *  the user can override, e.g. hide both once the assistant files things well on its own. */
  teachVisible: boolean;
  setTeachVisible: (v: boolean) => void;
  /** Whether the **Map** tab is shown. A Depth-keyed reveal the user can override, on the same
   *  footing as the learning tools above. */
  mapVisible: boolean;
  setMapVisible: (v: boolean) => void;
  /** True when every appearance axis (System, Mode, Accent, Depth, Location, Teach) is at its
   *  out-of-the-box default — drives whether Settings offers an appearance "Reset". */
  appearanceIsDefault: boolean;
  /** Restore every appearance axis to its default and forget the per-System accent memory (#445). */
  resetAppearance: () => void;

  // Accessibility axes (opt-in; see the FontScale block above).
  /** Whole-UI text size. Surfaced in both the Appearance section and the Accessibility tab. */
  fontScale: FontScale;
  setFontScale: (v: FontScale) => void;
  /** Force reduced motion regardless of the OS setting (false = follow the OS). */
  reduceMotion: boolean;
  setReduceMotion: (v: boolean) => void;
  /** Use Atkinson Hyperlegible for UI + heading text. */
  legibleFont: boolean;
  setLegibleFont: (v: boolean) => void;
  /** Control density / touch-target size. `standard` (WCAG 2.5.8) is the fresh-install default;
   *  existing installs are pinned to `compact` (today's sizing) by a one-time migration, so their
   *  look is undisturbed until they choose otherwise or Reset. */
  density: Density;
  setDensity: (v: Density) => void;
  /** Swap the categorical palettes (graph nodes, calendar sources) and semantic status colours for a
   *  colour-blind-safe (Okabe–Ito) set. Off by default (= today), so it needs no migration. */
  colorblind: boolean;
  setColorblind: (v: boolean) => void;
  /** Contrast level. `aa` (WCAG 1.4.3) is the fresh-install default; existing installs are pinned to
   *  `legacy` (today's ramps) by the same one-time migration as density; `high` reaches AAA. */
  contrast: Contrast;
  setContrast: (v: Contrast) => void;
  /** True when every accessibility axis is at its default — drives the Accessibility tab's "Reset". */
  accessibilityIsDefault: boolean;
  /** Restore every accessibility axis (text size, contrast, density, motion, legible font, colour-
   *  blind palette) to its default — including the compliant baseline for the legacy-pinned axes. */
  resetAccessibility: () => void;
}

// App defaults. Slate + Dark + its default accent (the Eigengrau monochrome — ACCENTS.slate[0])
// is the out-of-the-box look; existing installs keep whatever they've persisted.
const DEFAULT_SYSTEM: System = "slate";
const DEFAULT_MODE_PREF: ModePref = "dark";
const DEFAULT_DEPTH: Depth = "standard";

// Accessibility axes (opt-in). Their defaults equal today's behaviour — font size at 100%, motion
// following the OS, the theme's own fonts — so existing users see no change and no migration is
// needed. Persisted alongside the visual theme (localStorage + the mirrored `appearance` blob).
export type FontScale = "small" | "default" | "large" | "xlarge";
const FONT_SCALE_VALUES: Record<FontScale, number> = {
  small: 0.9,
  default: 1,
  large: 1.15,
  xlarge: 1.3,
};
const FONT_SCALES: readonly FontScale[] = ["small", "default", "large", "xlarge"];
const DEFAULT_FONT_SCALE: FontScale = "default";

// Density is the one accessibility axis whose compliant default differs from today's behaviour, so
// it carries a one-time legacy pin: a *fresh* install gets `standard` (meets WCAG 2.5.8's 24px target
// out of the box), while an *existing* install is pinned to `compact` (today's tight sizing) so the
// update disturbs nothing. The pin needs no dedicated flag — a first launch that already has theme
// state in localStorage (KEY.system) is by definition an existing user (see the density useState).
// Once persisted the stored value always wins. Because a pinned user then reads density !== default,
// the Accessibility tab surfaces its Reset, which restores the compliant `standard` — the "keep my
// look, or reset to the compliant baseline" contract.
const DEFAULT_DENSITY: Density = "standard";
const LEGACY_DENSITY: Density = "compact";

/** The density a fresh mount starts at. A stored value always wins; otherwise an install that
 *  already carries theme state (`hasThemeState`, i.e. a `pm:theme:system` value is present) is an
 *  existing user and is pinned to the legacy sizing, while a genuinely fresh install gets the
 *  compliant default. Pure + exported so the one-time migration is unit-testable without mounting
 *  the provider. */
export function initialDensity(stored: string | null, hasThemeState: boolean): Density {
  if (stored !== null) return oneOf(stored, DENSITIES, DEFAULT_DENSITY);
  return hasThemeState ? LEGACY_DENSITY : DEFAULT_DENSITY;
}

// Contrast is the second axis with a compliant default that differs from today, so it carries the
// same one-time legacy pin as density (see initialDensity): a fresh install gets `aa` (WCAG 1.4.3),
// an existing install is pinned to `legacy` (today's ramps) and can Reset up to the compliant baseline.
const DEFAULT_CONTRAST: Contrast = "aa";
const LEGACY_CONTRAST: Contrast = "legacy";

/** The contrast a fresh mount starts at — the density legacy-pin, applied to the contrast axis. Pure
 *  + exported for the same migration unit test. */
export function initialContrast(stored: string | null, hasThemeState: boolean): Contrast {
  if (stored !== null) return oneOf(stored, CONTRASTS, DEFAULT_CONTRAST);
  return hasThemeState ? LEGACY_CONTRAST : DEFAULT_CONTRAST;
}

// The Teach-tab visibility override: "auto" follows the Depth preset (hidden for minimalist,
// shown for standard/power); "show"/"hide" are explicit choices made from Settings.
type TeachPref = "auto" | "show" | "hide";
const TEACH_PREFS: readonly TeachPref[] = ["auto", "show", "hide"];
const DEFAULT_TEACH: TeachPref = "auto";

// The Map-tab visibility override, the same shape as Teach above: "auto" follows the Depth preset
// (hidden for minimalist, shown for standard/power), "show"/"hide" are explicit Settings choices.
// The semantic map is an exploratory surface rather than a daily one, so a minimalist preset has no
// use for it -- but someone on `min` who does want it can say so and keep it.
type MapPref = "auto" | "show" | "hide";
const MAP_PREFS: readonly MapPref[] = ["auto", "show", "hide"];
const DEFAULT_MAP: MapPref = "auto";

const KEY = {
  system: "pm:theme:system",
  mode: "pm:theme:mode", // legacy (pre-2.84): held "dark"|"light"; still read once for migration
  modePref: "pm:theme:modePref",
  autoLocation: "pm:theme:autoLocation",
  accent: "pm:theme:accent",
  depth: "pm:theme:depth",
  accentBySystem: "pm:theme:accentBySystem",
  teach: "pm:theme:teach",
  map: "pm:theme:map",
  fontScale: "pm:a11y:fontScale",
  reduceMotion: "pm:a11y:reduceMotion",
  legibleFont: "pm:a11y:legibleFont",
  density: "pm:a11y:density",
  colorblind: "pm:a11y:colorblind",
  contrast: "pm:a11y:contrast",
};

// localStorage can throw (locked-down webviews); never let a theme read/write crash the app.
function read(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}
function write(key: string, value: string): void {
  try {
    localStorage.setItem(key, value);
  } catch {
    /* ignore — theme just won't persist */
  }
}

function oneOf<T extends string>(value: string | null, allowed: readonly T[], fallback: T): T {
  return value !== null && allowed.includes(value as T) ? (value as T) : fallback;
}

// The stored Mode preference, migrating a pre-2.84 raw "dark"/"light" value (which is a valid
// preference) when the new key isn't set yet.
function readModePref(): ModePref {
  const p = read(KEY.modePref);
  if (p !== null) return oneOf(p, MODE_PREFS, DEFAULT_MODE_PREF);
  return oneOf(read(KEY.mode), MODE_PREFS, DEFAULT_MODE_PREF);
}

function readAccentBySystem(): Partial<Record<System, string>> {
  try {
    const raw = read(KEY.accentBySystem);
    return raw ? (JSON.parse(raw) as Partial<Record<System, string>>) : {};
  } catch {
    return {};
  }
}

function defaultAccentFor(system: System): string {
  return ACCENTS[system][0];
}

const ThemeContext = createContext<ThemeState | null>(null);

export function ThemeProvider({ children }: { children: ReactNode }) {
  const initialSystem = oneOf(read(KEY.system), SYSTEMS, DEFAULT_SYSTEM);

  const [system, setSystemState] = useState<System>(initialSystem);
  const [modePref, setModePrefState] = useState<ModePref>(readModePref);
  const [autoLocation, setAutoLocationState] = useState<string>(() => read(KEY.autoLocation) ?? "");
  const [depth, setDepthState] = useState<Depth>(() =>
    oneOf(read(KEY.depth), DEPTHS, DEFAULT_DEPTH),
  );
  const [accent, setAccentState] = useState<string>(() => {
    const stored = read(KEY.accent);
    return stored && ACCENTS[initialSystem].includes(stored)
      ? stored
      : defaultAccentFor(initialSystem);
  });
  const [teachPref, setTeachPrefState] = useState<TeachPref>(() =>
    oneOf(read(KEY.teach), TEACH_PREFS, DEFAULT_TEACH),
  );
  const [mapPref, setMapPrefState] = useState<MapPref>(() =>
    oneOf(read(KEY.map), MAP_PREFS, DEFAULT_MAP),
  );
  const [fontScale, setFontScale] = useState<FontScale>(() =>
    oneOf(read(KEY.fontScale), FONT_SCALES, DEFAULT_FONT_SCALE),
  );
  const [reduceMotion, setReduceMotion] = useState<boolean>(
    () => read(KEY.reduceMotion) === "true",
  );
  const [legibleFont, setLegibleFont] = useState<boolean>(() => read(KEY.legibleFont) === "true");
  // Reads KEY.system to tell an existing install from a fresh one — safe here because the persist
  // effect (which writes KEY.system) only runs after mount, so it still reflects the pre-upgrade state.
  const [density, setDensity] = useState<Density>(() =>
    initialDensity(read(KEY.density), read(KEY.system) !== null),
  );
  const [colorblind, setColorblind] = useState<boolean>(() => read(KEY.colorblind) === "true");
  const [contrast, setContrast] = useState<Contrast>(() =>
    initialContrast(read(KEY.contrast), read(KEY.system) !== null),
  );

  // The resolved Mode (+ how/where it was resolved). Computed synchronously for a themed first
  // paint, then kept live by the effect below (OS changes, sunrise/sunset, focus).
  const [resolution, setResolution] = useState<ModeResolution>(() =>
    resolveMode(readModePref(), new Date(), read(KEY.autoLocation)),
  );
  const mode = resolution.mode;

  // Was localStorage empty at boot? If so this is likely a fresh machine (or a
  // restored data folder), so the stored blob should win on hydration. Computed once,
  // before the persist effect writes anything back.
  const [bootEmpty] = useState(() => read(KEY.system) === null);
  // Gate the store-mirror until the one-shot hydration has run, so the default theme
  // can't overwrite a stored blob before we've read it (a fresh-machine race).
  const [hydrated, setHydrated] = useState(false);

  // Apply a blob read back from the store (theme axes + the per-System accent memory).
  function applyAppearance(blob: unknown): void {
    if (!blob || typeof blob !== "object") return;
    const b = blob as Record<string, unknown>;
    const sys = oneOf(typeof b.system === "string" ? b.system : null, SYSTEMS, DEFAULT_SYSTEM);
    if (b.accentBySystem && typeof b.accentBySystem === "object") {
      write(KEY.accentBySystem, JSON.stringify(b.accentBySystem));
    }
    setSystemState(sys);
    // Prefer the new `modePref`; fall back to a legacy raw `mode` from an older blob.
    const pref =
      typeof b.modePref === "string" ? b.modePref : typeof b.mode === "string" ? b.mode : null;
    setModePrefState(oneOf(pref, MODE_PREFS, DEFAULT_MODE_PREF));
    setAutoLocationState(typeof b.autoLocation === "string" ? b.autoLocation : "");
    setDepthState(oneOf(typeof b.depth === "string" ? b.depth : null, DEPTHS, DEFAULT_DEPTH));
    setAccentState(
      typeof b.accent === "string" && ACCENTS[sys].includes(b.accent)
        ? b.accent
        : defaultAccentFor(sys),
    );
    // Accessibility axes — absent from an older blob, which correctly hydrates them to their
    // (behaviour-preserving) defaults.
    setFontScale(
      oneOf(typeof b.fontScale === "string" ? b.fontScale : null, FONT_SCALES, DEFAULT_FONT_SCALE),
    );
    setReduceMotion(b.reduceMotion === true);
    setLegibleFont(b.legibleFont === true);
    // A blob without `density` predates this axis, so it belongs to an existing user whose folder was
    // restored on a fresh machine → pin the legacy sizing, matching the localStorage migration above.
    setDensity(oneOf(typeof b.density === "string" ? b.density : null, DENSITIES, LEGACY_DENSITY));
    setColorblind(b.colorblind === true);
    // A blob without `contrast` predates the axis → an existing user's folder → pin legacy, as above.
    setContrast(
      oneOf(typeof b.contrast === "string" ? b.contrast : null, CONTRASTS, LEGACY_CONTRAST),
    );
  }

  // One-shot hydration from the store. On a fresh machine (localStorage empty) the
  // stored blob is applied; either way we then unlock the store-mirror below.
  useEffect(() => {
    let cancelled = false;
    getPref(PREF_KEY)
      .then((raw) => {
        if (cancelled || !raw || !bootEmpty) return;
        try {
          applyAppearance(JSON.parse(raw));
        } catch {
          /* ignore a corrupt blob — keep the localStorage/default theme */
        }
      })
      .catch(() => {
        /* store not ready / no value — keep localStorage */
      })
      .finally(() => {
        if (!cancelled) setHydrated(true);
      });
    return () => {
      cancelled = true;
    };
  }, [bootEmpty]);

  // Keep the resolved Mode live. Explicit light/dark need no watchers; `system` follows the OS
  // light/dark query; `auto` re-resolves at each sunrise/sunset (a self-rescheduling timer) and,
  // as a safety net for sleep/resume where timers can be skipped, whenever the app regains focus.
  useEffect(() => {
    const apply = () => setResolution(resolveMode(modePref, new Date(), autoLocation));

    if (modePref === "light" || modePref === "dark") {
      apply();
      return;
    }

    let timer: ReturnType<typeof setTimeout> | undefined;
    let mql: MediaQueryList | undefined;
    const onChange = () => apply();

    // OS light/dark — drives `system`, and the fallback path of `auto` when we have no location.
    try {
      mql = window.matchMedia("(prefers-color-scheme: dark)");
      mql.addEventListener("change", onChange);
    } catch {
      /* matchMedia unavailable — explicit choices still work */
    }

    if (modePref === "system") {
      apply();
    } else {
      // auto: resolve now and schedule the next flip at the coming sunrise/sunset.
      const tick = () => {
        const res = resolveMode("auto", new Date(), autoLocation);
        setResolution(res);
        const ms = res.nextChange
          ? Math.max(1000, res.nextChange.getTime() - Date.now()) + 1000 // +1s so we're past it
          : 3_600_000; // polar day/night or no location → re-check hourly
        timer = setTimeout(tick, ms);
      };
      tick();
    }

    document.addEventListener("visibilitychange", onChange);
    window.addEventListener("focus", onChange);
    return () => {
      if (timer !== undefined) clearTimeout(timer);
      mql?.removeEventListener("change", onChange);
      document.removeEventListener("visibilitychange", onChange);
      window.removeEventListener("focus", onChange);
    };
  }, [modePref, autoLocation]);

  // Switching System recalls the accent last chosen for it, else that System's default (§3).
  function setSystem(next: System): void {
    setSystemState(next);
    const remembered = readAccentBySystem()[next];
    setAccentState(
      remembered && ACCENTS[next].includes(remembered) ? remembered : defaultAccentFor(next),
    );
  }
  function setAccent(next: string): void {
    setAccentState(next);
    const map = readAccentBySystem();
    map[system] = next;
    write(KEY.accentBySystem, JSON.stringify(map));
  }

  // The learning tools (Review + Teach) are a Depth-keyed feature reveal — hidden for minimalist,
  // shown for standard/power — until the user makes an explicit choice in Settings, which then wins.
  // Lets a user who's happy with the assistant's filing hide both once they no longer want to curate.
  const teachVisible = teachPref === "auto" ? depth !== "min" : teachPref === "show";
  function setTeachVisible(visible: boolean): void {
    const pref: TeachPref = visible ? "show" : "hide";
    setTeachPrefState(pref);
    write(KEY.teach, pref);
  }

  // The Map tab, on the same footing (see MapPref above).
  const mapVisible = mapPref === "auto" ? depth !== "min" : mapPref === "show";
  function setMapVisible(visible: boolean): void {
    const pref: MapPref = visible ? "show" : "hide";
    setMapPrefState(pref);
    write(KEY.map, pref);
  }

  // Whether the appearance is untouched from the defaults (the accent default is System-relative, so
  // it's compared against the default System's default accent). Drives the Settings "Reset" affordance.
  const appearanceIsDefault =
    system === DEFAULT_SYSTEM &&
    modePref === DEFAULT_MODE_PREF &&
    depth === DEFAULT_DEPTH &&
    accent === defaultAccentFor(DEFAULT_SYSTEM) &&
    autoLocation === "" &&
    teachPref === DEFAULT_TEACH &&
    mapPref === DEFAULT_MAP;

  const accessibilityIsDefault =
    fontScale === DEFAULT_FONT_SCALE &&
    !reduceMotion &&
    !legibleFont &&
    density === DEFAULT_DENSITY &&
    !colorblind &&
    contrast === DEFAULT_CONTRAST;

  function resetAccessibility(): void {
    setFontScale(DEFAULT_FONT_SCALE);
    setReduceMotion(false);
    setLegibleFont(false);
    setDensity(DEFAULT_DENSITY);
    setColorblind(false);
    setContrast(DEFAULT_CONTRAST);
  }

  // Restore every axis to its default in one go. The persist effect below mirrors the axis changes to
  // localStorage + the stored `appearance` blob; teach and the per-System accent memory aren't in that
  // effect, so they're written here directly. `setSystemState` (not the public `setSystem`) avoids
  // re-recording a per-System accent while we're clearing that memory.
  function resetAppearance(): void {
    write(KEY.accentBySystem, "{}");
    write(KEY.teach, DEFAULT_TEACH);
    write(KEY.map, DEFAULT_MAP);
    setSystemState(DEFAULT_SYSTEM);
    setModePrefState(DEFAULT_MODE_PREF);
    setDepthState(DEFAULT_DEPTH);
    setAccentState(defaultAccentFor(DEFAULT_SYSTEM));
    setAutoLocationState("");
    setTeachPrefState(DEFAULT_TEACH);
    setMapPrefState(DEFAULT_MAP);
  }

  // Apply + persist whenever an axis (or the resolved Mode) changes (also runs on mount → themed
  // first paint). localStorage is the fast path; the store is mirrored once hydration has run so the
  // theme survives a folder backup/transfer. We persist the *preference*, not the resolved Mode.
  useEffect(() => {
    applyTheme(document.documentElement, system, mode, accent, depth, {
      fontScale: FONT_SCALE_VALUES[fontScale],
      reduceMotion,
      legibleFont,
      density,
      colorblind,
      contrast,
    });
    write(KEY.system, system);
    write(KEY.modePref, modePref);
    write(KEY.autoLocation, autoLocation);
    write(KEY.accent, accent);
    write(KEY.depth, depth);
    write(KEY.fontScale, fontScale);
    write(KEY.reduceMotion, String(reduceMotion));
    write(KEY.legibleFont, String(legibleFont));
    write(KEY.density, density);
    write(KEY.colorblind, String(colorblind));
    write(KEY.contrast, contrast);
    if (hydrated) {
      const blob = {
        system,
        modePref,
        autoLocation,
        accent,
        depth,
        accentBySystem: readAccentBySystem(),
        fontScale,
        reduceMotion,
        legibleFont,
        density,
        colorblind,
        contrast,
      };
      setPref(PREF_KEY, JSON.stringify(blob)).catch(() => {
        /* fire-and-forget — localStorage already holds the value */
      });
    }
  }, [
    system,
    mode,
    modePref,
    autoLocation,
    accent,
    depth,
    fontScale,
    reduceMotion,
    legibleFont,
    density,
    colorblind,
    contrast,
    hydrated,
  ]);

  const value: ThemeState = {
    system,
    mode,
    modePref,
    modeSource: resolution.source,
    modeCoords: resolution.coords,
    modeNextChange: resolution.nextChange,
    accent,
    depth,
    autoLocation,
    setSystem,
    setModePref: setModePrefState,
    setAccent,
    setDepth: setDepthState,
    setAutoLocation: setAutoLocationState,
    teachVisible,
    setTeachVisible,
    mapVisible,
    setMapVisible,
    appearanceIsDefault,
    resetAppearance,
    fontScale,
    setFontScale,
    reduceMotion,
    setReduceMotion,
    legibleFont,
    setLegibleFont,
    density,
    setDensity,
    colorblind,
    setColorblind,
    contrast,
    setContrast,
    accessibilityIsDefault,
    resetAccessibility,
  };

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}

export function useTheme(): ThemeState {
  const ctx = useContext(ThemeContext);
  if (!ctx) throw new Error("useTheme must be used within <ThemeProvider>");
  return ctx;
}
