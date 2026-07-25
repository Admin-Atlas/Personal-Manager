// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The one daily briefing, shared by every surface that shows it.
//
// The briefing used to belong to the Focus tab, held in a module-scope cache there. That was fine
// while Focus was its only reader. It stops being fine the moment a second surface exists, because
// the Sidebar renders in EVERY view: a sidebar panel and the Focus card are mounted AT THE SAME TIME
// whenever the user is on Focus. Two independent copies of the old logic would each run its own
// mount effect, each call `getDailyBriefing()`, and each hit the "if stale, regenerate" branch —
// and `refresh_daily_briefing` has no single-flight guard on the backend. Two concurrent refreshes
// mean two model calls, two usage-log rows, and a last-write-wins race on the two settings keys that
// can pair an OLDER body with a NEWER timestamp. A Refresh clicked in one surface also wouldn't
// reach the other, because the module cache was only read in a `useState` initialiser.
//
// So this is a provider, mirroring `reader.tsx`: one piece of state, one in-flight latch, one set of
// subscribers. Every surface renders `<Briefing>` against `useBriefing()` and they cannot disagree.

import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useRef,
  useState,
  type ReactNode,
} from "react";
import type { DailyBriefing } from "./types";
import { getDailyBriefing, refreshDailyBriefing } from "./ipc";

interface BriefingState {
  /** The current briefing, or null before the first load (and if the load failed). */
  briefing: DailyBriefing | null;
  /** True while a regeneration is in flight — drives every surface's Refresh affordance at once. */
  busy: boolean;
  /** Regenerate from the current projects, flags and calendar. Best-effort: a missing key or a model
   *  hiccup leaves the previous briefing in place. Concurrent calls share the one in-flight request
   *  rather than starting a second model call. */
  refresh: () => Promise<void>;
}

const BriefingContext = createContext<BriefingState | null>(null);

export function BriefingProvider({ children }: { children: ReactNode }) {
  const [briefing, setBriefing] = useState<DailyBriefing | null>(null);
  const [busy, setBusy] = useState(false);

  // These calls resolve after the user may have navigated away or the app may have unmounted; don't
  // write state then. Re-armed on mount because StrictMode double-invokes effects in dev
  // (mount → unmount → mount) and a flag left false would silently drop every later write.
  const aliveRef = useRef(true);
  useEffect(() => {
    aliveRef.current = true;
    return () => void (aliveRef.current = false);
  }, []);

  // The single-flight latch. Held as the in-flight PROMISE rather than a boolean so a second caller
  // awaits the same regeneration instead of returning early while the first is still running — which
  // matters for `onFlagResolved`, whose `Promise.all` must not resolve before the briefing lands.
  // Assigned BEFORE the first await, so StrictMode's double mount cannot slip a second call past it.
  const inFlight = useRef<Promise<void> | null>(null);

  const refresh = useCallback(() => {
    if (inFlight.current) return inFlight.current;
    setBusy(true);
    const run = (async () => {
      try {
        const next = await refreshDailyBriefing();
        if (aliveRef.current) setBriefing(next);
      } catch {
        /* keep whatever we have — the briefing is optional */
      } finally {
        inFlight.current = null;
        if (aliveRef.current) setBusy(false);
      }
    })();
    inFlight.current = run;
    return run;
  }, []);

  // Load the stored briefing once at app scope, and silently regenerate when it's stale (the backend
  // computes `stale` against its own freshness window), so it refreshes about once a day on open
  // rather than on every mount. Runs here instead of per-surface, so turning a surface on or off
  // never triggers a model call.
  useEffect(() => {
    void (async () => {
      try {
        const stored = await getDailyBriefing();
        if (!aliveRef.current) return;
        setBriefing(stored);
        if (stored.stale) void refresh();
      } catch {
        /* the briefing is optional — every surface renders without it */
      }
    })();
  }, [refresh]);

  return (
    <BriefingContext.Provider value={{ briefing, busy, refresh }}>
      {children}
    </BriefingContext.Provider>
  );
}

export function useBriefing(): BriefingState {
  const ctx = useContext(BriefingContext);
  if (!ctx) throw new Error("useBriefing must be used within <BriefingProvider>");
  return ctx;
}
