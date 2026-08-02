// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useRef, useState } from "react";
import type { UnlistenFn } from "@tauri-apps/api/event";
import type { SyncEvent, SyncReport } from "./types";

/**
 * The **detached index-only sync** shared by the Drive, OneDrive, and local-folder connectors. All
 * three run the identical lifecycle: a background sync that keeps going if you leave Settings,
 * single-flighted (a second "Sync now" mid-run is folded into a follow-up pass and shows "Queued"),
 * stop-able (keeps everything indexed so far), with progress + a post-sync report restored from the
 * backend snapshot on (re)mount. This hook owns that whole state machine once; each connector injects
 * its provider-specific IPC and renders its own shell (scope pickers, sign-in, identity model).
 */

/** The common fields of every connector's in-flight sync snapshot (each also carries its own target
 *  field — `account` or `folder` — read via {@link DetachedSyncOptions.targetOf}). */
export interface DetachedSyncSnapshot {
  running: boolean;
  processed: number;
  total: number | null;
  /** Epoch ms the running pass began, for an elapsed timer that survives this view unmounting. */
  started_at_ms: number | null;
  last_report: SyncReport | null;
  /** Whether a Stop has already been requested for the running pass. The backend derives it from the
   *  cancel flag, so it is the one owner of that fact — this view only reflects it. */
  stopping: boolean;
}

export interface DetachedSyncOptions<S extends DetachedSyncSnapshot> {
  /** Subscribe to the connector's global progress events; returns the unlisten handle. */
  subscribe: (cb: (ev: SyncEvent) => void) => Promise<UnlistenFn>;
  /** Fetch the current background-sync snapshot (to restore progress / the last report on mount). */
  fetchStatus: () => Promise<S>;
  /** Pull the "which item is syncing" target out of a snapshot (`s.account` or `s.folder`). */
  targetOf: (snapshot: S) => string | null;
  /** Start a background sync for one target (or all when null). */
  start: (target: string | null) => Promise<unknown>;
  /** Ask the running sync to stop after the current file. */
  stop: () => Promise<unknown>;
  /** Called after a sync finishes (and on any {@link watch} event) so the connector refetches its list. */
  onSettled: () => void;
  /** Optional extra subscription — the local-folder watcher's `local://changed`, which also refetches. */
  watch?: (cb: () => void) => Promise<UnlistenFn>;
}

export interface DetachedSync {
  /** The label of an in-flight connect/disconnect/add action, or null. */
  busy: string | null;
  error: string | null;
  setError: (e: string | null) => void;
  /** Run a labelled one-shot action (connect / disconnect / add): sets `busy`, clears `error`, catches. */
  run: (label: string, fn: () => Promise<void>) => Promise<void>;
  /** Live progress of the running sync, or null when idle. */
  progress: { processed: number; total: number | null } | null;
  /** Epoch ms the running sync began, or null. Kept OUT of `progress` on purpose: the `counted` /
   *  `item` handlers replace that object wholesale on every file, so a start stamp folded into it
   *  would be dropped by the next event — the exact class of bug this whole change is fixing. */
  startedAt: number | null;
  /** `progress != null` — a sync is on screen. */
  syncing: boolean;
  /** The target (account email / folder key) currently syncing, or null for an all-targets pass. */
  target: string | null;
  /** Targets queued mid-run (folded into the backend's follow-up pass) — their row shows "Queued". */
  queued: Set<string>;
  /** The most recent finished sync's report, or null. */
  report: SyncReport | null;
  dismissReport: () => void;
  /** True between pressing Stop and the "finished" event arriving. */
  stopping: boolean;
  confirmStop: boolean;
  setConfirmStop: (v: boolean) => void;
  /** Start (or queue) a sync for one target (or all when null). */
  sync: (target: string | null) => void;
  /** Ask the running sync to stop. */
  requestStop: () => void;
}

export function useDetachedSync<S extends DetachedSyncSnapshot>(
  opts: DetachedSyncOptions<S>,
): DetachedSync {
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [progress, setProgress] = useState<{ processed: number; total: number | null } | null>(
    null,
  );
  const [startedAt, setStartedAt] = useState<number | null>(null);
  const [target, setTarget] = useState<string | null>(null);
  const [queued, setQueued] = useState<Set<string>>(new Set());
  const [report, setReport] = useState<SyncReport | null>(null);
  const [stopping, setStopping] = useState(false);
  const [confirmStop, setConfirmStop] = useState(false);
  // Live mirror of "a sync is on screen", so a fire-and-forget "Sync now" from an earlier render (the
  // connect tail, which may fire mid-sync) can tell whether it owns the visible bar without hijacking
  // a running sync's progress.
  const syncingRef = useRef(false);
  // Latest options in a ref, so the mount-once subscription always calls the current closures (the
  // caller passes fresh inline functions each render) without re-subscribing every render.
  const optsRef = useRef(opts);
  useEffect(() => {
    optsRef.current = opts;
  });

  useEffect(() => {
    syncingRef.current = progress != null;
  }, [progress]);

  // Subscribe to the detached sync's global progress events, and — if a sync is already in flight when
  // this view (re)mounts — restore the bar from the backend snapshot so it never looks stalled. If it
  // already finished, show the last report so a returning user still sees the result. Mount-once: the
  // IPC bindings are read from `optsRef`, so the subscription is set up and torn down exactly once.
  useEffect(() => {
    let mounted = true;
    const unlistenP = optsRef.current.subscribe((ev) => {
      if (!mounted) return;
      if (ev.type === "counted") {
        setProgress({ processed: 0, total: ev.total });
        // Each PASS announces its own target, and a run sweeps the targets queued mid-run one at a
        // time — so this is where a row flips "Queued" → "Syncing…". Without it the backend moved on
        // to the queued account while its row sat on "Queued" for the rest of the run.
        const tgt = ev.target;
        setTarget(tgt);
        setQueued((q) => {
          if (q.size === 0) return q;
          // An all-targets sweep covers everything that was waiting, so nothing stays queued.
          if (tgt == null) return new Set();
          if (!q.has(tgt)) return q;
          const next = new Set(q);
          next.delete(tgt);
          return next;
        });
      } else if (ev.type === "item") setProgress({ processed: ev.processed, total: ev.total });
      else if (ev.type === "finished") {
        setProgress(null);
        setStartedAt(null);
        setTarget(null);
        setQueued(new Set());
        setStopping(false);
        // Clear any unconfirmed Stop prompt: the sync is over, so "Stop indexing?" is moot. Without
        // this it would stay latent (the dialog now lives in SyncProgress, which unmounts on finish)
        // and pop spuriously over the NEXT sync. (The old always-mounted dialog self-healed on dismiss.)
        setConfirmStop(false);
        setReport(ev.report);
        optsRef.current.onSettled();
      }
    });
    // The live watcher applied a batch of on-disk changes outside a manual sync — refetch so counts
    // and state badges reflect it (local folders only; cheap — the list is small).
    const watchP = optsRef.current.watch?.(() => {
      if (mounted) optsRef.current.onSettled();
    });
    void optsRef.current
      .fetchStatus()
      .then((s) => {
        if (!mounted) return;
        if (s.running) {
          setProgress({ processed: s.processed, total: s.total });
          // The whole point: a bar restored on remount counts from when the BACKEND started, so
          // leaving Settings mid-sync and coming back no longer restarts the timer at 0:00.
          setStartedAt(s.started_at_ms);
          setTarget(optsRef.current.targetOf(s));
          // …and the same for the Stop already pressed (#699). This was the one piece of the run's
          // state the remount did NOT restore, so a tab switch mid-stop brought back a frozen bar
          // beside a button that read "Stop indexing" again — the bar restored from the backend, the
          // button from local state that had just been thrown away.
          setStopping(s.stopping);
        } else if (s.last_report) {
          setReport(s.last_report);
        }
      })
      .catch(() => {});
    return () => {
      mounted = false;
      void unlistenP.then((fn) => fn());
      void watchP?.then((fn) => fn());
    };
  }, []);

  const run = useCallback(async (label: string, fn: () => Promise<void>) => {
    setBusy(label);
    setError(null);
    try {
      await fn();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }, []);

  // Start a background sync for one target (or all). Fire-and-forget: progress arrives via the global
  // listener above, and the sync survives navigating away. The backend single-flights, so a request
  // made while one is already running is folded into a follow-up pass — only the call that *starts* a
  // sync drives the optimistic progress + error rollback, so it never hijacks a running bar.
  const sync = useCallback((tgt: string | null) => {
    setError(null);
    const startsIt = !syncingRef.current;
    if (startsIt) {
      setReport(null);
      setTarget(tgt);
      setQueued(new Set());
      setProgress({ processed: 0, total: null });
      // Optimistic — the backend stamps its own on `begin_pass`, and the next remount reads that.
      setStartedAt(Date.now());
    } else if (tgt != null) {
      setQueued((q) => new Set(q).add(tgt));
    }
    void optsRef.current.start(tgt).catch((e) => {
      if (startsIt) {
        setError(String(e));
        setProgress(null);
        setStartedAt(null);
        setTarget(null);
      }
    });
  }, []);

  // Stop the running sync (the caller gates this behind a confirm). The backend halts after the current
  // file and keeps everything indexed so far; the "finished" event (a `cancelled` report) clears the bar.
  const requestStop = useCallback(() => {
    setStopping(true);
    void optsRef.current.stop().catch(() => setStopping(false));
  }, []);

  const dismissReport = useCallback(() => setReport(null), []);

  return {
    busy,
    error,
    setError,
    run,
    progress,
    startedAt,
    syncing: progress != null,
    target,
    queued,
    report,
    dismissReport,
    stopping,
    confirmStop,
    setConfirmStop,
    sync,
    requestStop,
  };
}
