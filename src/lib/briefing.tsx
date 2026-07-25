// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The one daily briefing, shared by every surface that shows it.
//
// The briefing used to belong to the Focus tab, held in a module-scope cache there. That was fine
// while Focus was its only reader. It stops being fine the moment a second surface exists, because
// the Sidebar renders in EVERY view: a sidebar panel and the Focus card are mounted AT THE SAME TIME
// whenever the user is on Focus. Two independent copies of the old logic would each run its own
// mount effect, each call `getDailyBriefing()`, and each hit the "if stale, regenerate" branch —
// two model calls, two usage-log rows, and a last-write-wins race on the stored keys that can pair
// an OLDER body with a NEWER timestamp. A Refresh clicked in one surface also wouldn't reach the
// other, because the module cache was only read in a `useState` initialiser.
//
// So this is a provider, mirroring `reader.tsx`: one piece of state, one in-flight latch, one set of
// subscribers. Every surface renders `<Briefing>` against `useBriefing()` and they cannot disagree.
//
// This latch covers ONE webview. Since #540 the same guarantee holds ACROSS windows — the backend
// command is single-flighted too — so the two are layered rather than duplicated: this one keeps a
// window's own surfaces in step and drives their shared busy state, and the backend one stops the
// main window, the always-on-top window and the schedulers from overlapping.

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
import {
  getDailyBriefing,
  onBriefingUpdated,
  refreshDailyBriefing,
  syncDailyBriefing,
} from "./ipc";

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

export function BriefingProvider({
  children,
  autoRefresh = true,
}: {
  children: ReactNode;
  /** Whether this provider runs the launch check. The main window's provider owns it; the
   *  always-on-top window passes false and simply follows `briefing://updated`. Since #540 a second
   *  check would be harmless (the backend folds it into the running one), but a display-only window
   *  has no business deciding when the model runs -- it shows what the app decided. */
  autoRefresh?: boolean;
}) {
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

  // `force` separates the user asking (the Refresh button, always regenerates) from the app
  // checking (the launch check, which regenerates only if the facts moved). Both go through the
  // one latch, and the backend applies the same distinction again across windows.
  const run = useCallback((force: boolean) => {
    if (inFlight.current) return inFlight.current;
    setBusy(true);
    const started = (async () => {
      try {
        const next = force ? await refreshDailyBriefing() : await syncDailyBriefing();
        if (aliveRef.current) setBriefing(next);
      } catch {
        /* keep whatever we have — the briefing is optional */
      } finally {
        inFlight.current = null;
        if (aliveRef.current) setBusy(false);
      }
    })();
    inFlight.current = started;
    return started;
  }, []);

  const refresh = useCallback(() => run(true), [run]);

  // Load the stored briefing once at app scope, then run the LAUNCH CHECK: the backend rebuilds the
  // facts and regenerates only if they differ from the ones the stored briefing was written from.
  // That covers the app having been closed for a day (dates move, so the facts move) without
  // spending a model call on a relaunch five minutes later. Runs here instead of per-surface, so
  // turning a surface on or off never triggers one either.
  useEffect(() => {
    void (async () => {
      try {
        // Paint the stored briefing first — the check may take a model call's worth of seconds.
        const stored = await getDailyBriefing();
        if (!aliveRef.current) return;
        setBriefing(stored);
        if (autoRefresh) void run(false);
      } catch {
        /* the briefing is optional — every surface renders without it */
      }
    })();
  }, [run, autoRefresh]);

  // Follow regenerations this provider didn't start: the hourly scheduler, an inputs-changed nudge
  // (calendar sync, milestone edit, flag resolved), or a Refresh clicked in the OTHER window. The
  // event carries no payload deliberately — every listener re-reads, so nobody renders a briefing
  // assembled from a stale event.
  useEffect(() => {
    const pending = onBriefingUpdated(() => {
      void getDailyBriefing()
        .then((next) => {
          if (aliveRef.current) setBriefing(next);
        })
        .catch(() => {
          /* optional, as everywhere else */
        });
    });
    return () => void pending.then((un) => un()).catch(() => {});
  }, []);

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
