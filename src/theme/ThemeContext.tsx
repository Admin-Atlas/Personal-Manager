// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The only state the design layer owns: { system, mode, accent, depth }. Persisted two ways:
// localStorage stays the fast-path so the very first paint is already themed (a synchronous
// read, no IPC round-trip → no flash), and it is *also* mirrored into the encrypted settings
// table via `set_pref` so the theme travels with the data folder when it's backed up or moved
// to another machine. On a fresh machine (localStorage empty at boot) we hydrate from the
// stored blob; on the same machine localStorage already holds the values, so nothing flashes
// and the store is simply refreshed.

import { createContext, useContext, useEffect, useState, type ReactNode } from "react";
import { applyTheme } from "./tokens";
import { ACCENTS, SYSTEMS, MODES, DEPTHS, type System, type Mode, type Depth } from "./profiles";
import { getPref, setPref } from "../lib/ipc";

// The settings-table key under which the full theme blob is mirrored (see below).
const PREF_KEY = "appearance";

export interface ThemeState {
  system: System;
  mode: Mode;
  accent: string;
  depth: Depth;
  setSystem: (s: System) => void;
  setMode: (m: Mode) => void;
  setAccent: (a: string) => void;
  setDepth: (d: Depth) => void;
}

const DEFAULT_SYSTEM: System = "editorial";
const DEFAULT_MODE: Mode = "dark";
const DEFAULT_DEPTH: Depth = "standard";

const KEY = {
  system: "pm:theme:system",
  mode: "pm:theme:mode",
  accent: "pm:theme:accent",
  depth: "pm:theme:depth",
  accentBySystem: "pm:theme:accentBySystem",
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
  const [mode, setModeState] = useState<Mode>(() => oneOf(read(KEY.mode), MODES, DEFAULT_MODE));
  const [depth, setDepthState] = useState<Depth>(() =>
    oneOf(read(KEY.depth), DEPTHS, DEFAULT_DEPTH),
  );
  const [accent, setAccentState] = useState<string>(() => {
    const stored = read(KEY.accent);
    return stored && ACCENTS[initialSystem].includes(stored)
      ? stored
      : defaultAccentFor(initialSystem);
  });

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
    setModeState(oneOf(typeof b.mode === "string" ? b.mode : null, MODES, DEFAULT_MODE));
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

  // Apply + persist whenever an axis changes (also runs on mount → themed first paint).
  // localStorage is the fast path; the store is mirrored once hydration has run so the
  // theme survives a folder backup/transfer.
  useEffect(() => {
    applyTheme(document.documentElement, system, mode, accent, depth);
    write(KEY.system, system);
    write(KEY.mode, mode);
    write(KEY.accent, accent);
    write(KEY.depth, depth);
    if (hydrated) {
      const blob = { system, mode, accent, depth, accentBySystem: readAccentBySystem() };
      setPref(PREF_KEY, JSON.stringify(blob)).catch(() => {
        /* fire-and-forget — localStorage already holds the value */
      });
    }
  }, [system, mode, accent, depth, hydrated]);

  const value: ThemeState = {
    system,
    mode,
    accent,
    depth,
    setSystem,
    setMode: setModeState,
    setAccent,
    setDepth: setDepthState,
  };

  return <ThemeContext.Provider value={value}>{children}</ThemeContext.Provider>;
}

export function useTheme(): ThemeState {
  const ctx = useContext(ThemeContext);
  if (!ctx) throw new Error("useTheme must be used within <ThemeProvider>");
  return ctx;
}
