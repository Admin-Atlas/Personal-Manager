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
  type System,
  type Mode,
  type ModePref,
  type Depth,
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
}

// App defaults. Slate + Dark + its default accent (the Eigengrau monochrome — ACCENTS.slate[0])
// is the out-of-the-box look; existing installs keep whatever they've persisted.
const DEFAULT_SYSTEM: System = "slate";
const DEFAULT_MODE_PREF: ModePref = "dark";
const DEFAULT_DEPTH: Depth = "standard";

// The Teach-tab visibility override: "auto" follows the Depth preset (hidden for minimalist,
// shown for standard/power); "show"/"hide" are explicit choices made from Settings.
type TeachPref = "auto" | "show" | "hide";
const TEACH_PREFS: readonly TeachPref[] = ["auto", "show", "hide"];
const DEFAULT_TEACH: TeachPref = "auto";

const KEY = {
  system: "pm:theme:system",
  mode: "pm:theme:mode", // legacy (pre-2.84): held "dark"|"light"; still read once for migration
  modePref: "pm:theme:modePref",
  autoLocation: "pm:theme:autoLocation",
  accent: "pm:theme:accent",
  depth: "pm:theme:depth",
  accentBySystem: "pm:theme:accentBySystem",
  teach: "pm:theme:teach",
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

  // Apply + persist whenever an axis (or the resolved Mode) changes (also runs on mount → themed
  // first paint). localStorage is the fast path; the store is mirrored once hydration has run so the
  // theme survives a folder backup/transfer. We persist the *preference*, not the resolved Mode.
  useEffect(() => {
    applyTheme(document.documentElement, system, mode, accent, depth);
    write(KEY.system, system);
    write(KEY.modePref, modePref);
    write(KEY.autoLocation, autoLocation);
    write(KEY.accent, accent);
    write(KEY.depth, depth);
    if (hydrated) {
      const blob = {
        system,
        modePref,
        autoLocation,
        accent,
        depth,
        accentBySystem: readAccentBySystem(),
      };
      setPref(PREF_KEY, JSON.stringify(blob)).catch(() => {
        /* fire-and-forget — localStorage already holds the value */
      });
    }
  }, [system, mode, modePref, autoLocation, accent, depth, hydrated]);

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
  };

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}

export function useTheme(): ThemeState {
  const ctx = useContext(ThemeContext);
  if (!ctx) throw new Error("useTheme must be used within <ThemeProvider>");
  return ctx;
}
