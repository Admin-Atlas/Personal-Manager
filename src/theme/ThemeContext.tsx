// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The only state the design layer owns: { system, mode, accent, depth }. Persisted in
// localStorage (not the backend Settings struct) — it's non-secret presentation state, reads
// synchronously so the first paint is already themed, and the read/write seam here can later
// move to IPC without touching any component.

import {
  createContext, useContext, useEffect, useState, type ReactNode,
} from "react";
import { applyTheme } from "./tokens";
import {
  ACCENTS, SYSTEMS, MODES, DEPTHS,
  type System, type Mode, type Depth,
} from "./profiles";

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
  const [depth, setDepthState] = useState<Depth>(() => oneOf(read(KEY.depth), DEPTHS, DEFAULT_DEPTH));
  const [accent, setAccentState] = useState<string>(() => {
    const stored = read(KEY.accent);
    return stored && ACCENTS[initialSystem].includes(stored)
      ? stored
      : defaultAccentFor(initialSystem);
  });

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
  useEffect(() => {
    applyTheme(document.documentElement, system, mode, accent, depth);
    write(KEY.system, system);
    write(KEY.mode, mode);
    write(KEY.accent, accent);
    write(KEY.depth, depth);
  }, [system, mode, accent, depth]);

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
