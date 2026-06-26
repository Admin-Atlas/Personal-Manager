// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The capability layer: the ONE source of truth for the two "developer" signals, so feature
// code never reads `import.meta.env.DEV` directly nor smears a bare `if (devMode)` through render
// logic (issue #78). Two distinct signals for two audiences:
//
//   * `useDevMode()` — RUNTIME, user-flippable, persisted. The MASTER switch for every developer
//     surface: when off, nothing developer-facing shows, in any build. Gates the ship-safe,
//     read-only INSPECTION surfaces directly. A normal supported setting (default off), NOT a
//     build-time gate — so it ships in the release and can be switched on by a curious user.
//   * `isDevBuild`  — BUILD-TIME (`import.meta.env.DEV`), re-exposed here. The hard FLOOR under the
//     maintainer-only TEST HARNESSES that must never ship (they write synthetic state): they are
//     dead-code-eliminated from release bundles. Harnesses gate on `isDevBuild && devMode` — the
//     build gate strips them from release, and the runtime toggle still hides them in a dev build.
//     This module is the single place that literal is read.
//
// devMode persists like the theme (ThemeContext): localStorage is the fast path so the Dev tab
// never flashes on first paint, mirrored into the encrypted `settings` table (`set_pref`) so it
// travels with the data folder.

import { createContext, useContext, useEffect, useState, type ReactNode } from "react";
import { getPref, setPref } from "./ipc";

/** Build-time signal: true only in a dev build. The ONE place `import.meta.env.DEV` is read. */
export const isDevBuild: boolean = import.meta.env.DEV;

const LS_KEY = "pm:dev:mode";
const PREF_KEY = "dev_mode";

interface CapabilityState {
  /** Runtime developer mode — gates the read-only inspection surfaces. */
  devMode: boolean;
  setDevMode: (on: boolean) => void;
}

const CapabilityContext = createContext<CapabilityState | null>(null);

// localStorage can throw (locked-down webviews); never let a capability read/write crash the app.
function readLs(): boolean {
  try {
    return localStorage.getItem(LS_KEY) === "true";
  } catch {
    return false;
  }
}
function writeLs(on: boolean): void {
  try {
    localStorage.setItem(LS_KEY, on ? "true" : "false");
  } catch {
    /* ignore — devMode just won't persist on this device */
  }
}

export function CapabilityProvider({ children }: { children: ReactNode }) {
  const [devMode, setDevModeState] = useState<boolean>(readLs);
  // localStorage empty at boot ⇒ likely a fresh machine / restored folder, so the stored mirror
  // should win on hydration. Captured once, before any write-back.
  const [bootEmpty] = useState(() => {
    try {
      return localStorage.getItem(LS_KEY) === null;
    } catch {
      return true;
    }
  });

  // One-shot hydration from the settings mirror — only on a fresh machine, so a local choice is
  // never overridden. We never blind-write the mirror on mount (that would persist the default
  // "off" for everyone); it is written only on an explicit toggle below.
  useEffect(() => {
    let cancelled = false;
    getPref(PREF_KEY)
      .then((raw) => {
        if (cancelled || !bootEmpty || raw == null) return;
        const on = raw === "true";
        setDevModeState(on);
        writeLs(on); // make the next boot flash-free
      })
      .catch(() => {
        /* store not ready / no value — keep the localStorage/default */
      });
    return () => {
      cancelled = true;
    };
  }, [bootEmpty]);

  function setDevMode(on: boolean): void {
    setDevModeState(on);
    writeLs(on);
    // Mirror so it travels with the data folder (fire-and-forget — localStorage already has it).
    setPref(PREF_KEY, on ? "true" : "false").catch(() => {});
  }

  return (
    <CapabilityContext.Provider value={{ devMode, setDevMode }}>
      {children}
    </CapabilityContext.Provider>
  );
}

export function useDevMode(): CapabilityState {
  const ctx = useContext(CapabilityContext);
  if (!ctx) throw new Error("useDevMode must be used within <CapabilityProvider>");
  return ctx;
}
