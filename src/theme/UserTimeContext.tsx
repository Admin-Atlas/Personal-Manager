// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The app's single frontend source of truth for "where/when the user is": their IANA time zone and
// their coordinates. Before this, the calendar derived time locally, the theme's auto light/dark
// derived location on its own, and each could drift. This centralises both:
//
//   • timeZone — the backend `time_zone` setting (the civil-day source of truth `resolve_zone`
//     already uses server-side), falling back to the device zone until the async read resolves.
//   • coords   — the lat/long the solar features need, via the shared `coordsFor` (a user override
//     from the theme's `autoLocation`, else the device-timezone's representative point).
//
// The PRIMARY calendar grid deliberately stays device-local (a product decision) — this property is
// what the *extra* timezone columns and the sunrise/sunset "Day" range read, and it unifies the
// theme's auto-mode with the calendar so the location logic lives in one place.
//
// Mounted INSIDE ThemeProvider (it reads `autoLocation` via useTheme). It cannot sit above the theme:
// ThemeProvider is the root and resolves Mode synchronously for a themed first paint, so the theme
// keeps owning `autoLocation` and reads coords through the same pure `coordsFor` — no upward
// dependency, no regression.

import { createContext, useContext, useEffect, useMemo, useState, type ReactNode } from "react";
import { getSettings } from "../lib/ipc";
import { useTheme } from "./ThemeContext";
import { coordsFor, deviceTimeZone, type Coords } from "./timezones";

export interface UserTime {
  /** The user's effective IANA time zone (backend setting, or the device zone as fallback). */
  timeZone: string;
  /** The user's effective coordinates for solar math, or null when unknown. */
  coords: Coords | null;
}

const UserTimeContext = createContext<UserTime | null>(null);

export function UserTimeProvider({ children }: { children: ReactNode }) {
  const { autoLocation } = useTheme();
  // Synchronous device-zone fallback for the first paint; overwritten by the stored setting once the
  // async read resolves. They normally coincide — the app writes the device zone into `time_zone` on
  // first run — so the brief fallback is correct in practice, and the grid is device-local regardless.
  const [timeZone, setTimeZone] = useState<string>(() => deviceTimeZone());

  useEffect(() => {
    let cancelled = false;
    const refresh = () => {
      getSettings()
        .then((s) => {
          if (!cancelled && s.time_zone) setTimeZone(s.time_zone);
        })
        .catch(() => {
          /* store not ready — keep the device fallback */
        });
    };
    refresh();
    // A zone change in Settings dispatches `pm:settings-changed`; refocus re-reads as a safety net.
    window.addEventListener("focus", refresh);
    window.addEventListener("pm:settings-changed", refresh);
    return () => {
      cancelled = true;
      window.removeEventListener("focus", refresh);
      window.removeEventListener("pm:settings-changed", refresh);
    };
  }, []);

  // Reactive because `autoLocation` comes from the theme context; the same pure derivation the
  // theme's auto-mode uses.
  const coords = useMemo(() => coordsFor(autoLocation), [autoLocation]);

  const value = useMemo<UserTime>(() => ({ timeZone, coords }), [timeZone, coords]);
  return <UserTimeContext.Provider value={value}>{children}</UserTimeContext.Provider>;
}

export function useUserTime(): UserTime {
  const ctx = useContext(UserTimeContext);
  if (!ctx) throw new Error("useUserTime must be used within <UserTimeProvider>");
  return ctx;
}
