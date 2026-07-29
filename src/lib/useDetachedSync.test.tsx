// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The shared detached-sync state machine that Drive, OneDrive and the local-folder connector all run
// (X-D2 consolidated the three shells onto it, with parity caught by read-review only — this is the
// test net). The hook is dependency-injected, so every connector interaction is exercised here with
// plain vi.fn()s and a captured event callback — no Tauri, no rendered connector shell.

import { act, renderHook, waitFor } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";
import { useDetachedSync, type DetachedSyncOptions } from "./useDetachedSync";
import type { DriveSyncState, SyncEvent, SyncReport } from "./types";

const REPORT: SyncReport = {
  indexed: 3,
  updated: 1,
  removed: 0,
  skipped: 0,
  failed: 0,
  cancelled: false,
  issues: [],
  issues_truncated: false,
};

const idle = (over: Partial<DriveSyncState> = {}): DriveSyncState => ({
  running: false,
  processed: 0,
  total: null,
  started_at_ms: null,
  account: null,
  last_report: null,
  ...over,
});

// A controllable fake of the injected connector IPC: the subscribe callback is captured so a test can
// push global sync events, and the vi.fn()s are tunable per test.
function makeOpts(over: Partial<DetachedSyncOptions<DriveSyncState>> = {}) {
  let cb: (ev: SyncEvent) => void = () => {};
  const unlisten = vi.fn();
  const opts: DetachedSyncOptions<DriveSyncState> = {
    subscribe: vi.fn(async (fn: (ev: SyncEvent) => void) => {
      cb = fn;
      return unlisten;
    }),
    fetchStatus: vi.fn(async () => idle()),
    targetOf: (s) => s.account,
    start: vi.fn(async () => undefined),
    stop: vi.fn(async () => undefined),
    onSettled: vi.fn(),
    ...over,
  };
  const emit = (ev: SyncEvent) => act(() => cb(ev));
  return { opts, unlisten, emit };
}

describe("useDetachedSync", () => {
  it("subscribes once on mount and starts idle", async () => {
    const { opts } = makeOpts();
    const { result } = renderHook(() => useDetachedSync(opts));
    await waitFor(() => expect(opts.fetchStatus).toHaveBeenCalledTimes(1));
    expect(opts.subscribe).toHaveBeenCalledTimes(1);
    expect(result.current.syncing).toBe(false);
    expect(result.current.progress).toBeNull();
    expect(result.current.busy).toBeNull();
  });

  it("restores an in-flight sync from the mount snapshot", async () => {
    const { opts } = makeOpts({
      fetchStatus: vi.fn(async () =>
        idle({ running: true, processed: 5, total: 10, account: "a@b.com" }),
      ),
    });
    const { result } = renderHook(() => useDetachedSync(opts));
    await waitFor(() => expect(result.current.syncing).toBe(true));
    expect(result.current.progress).toEqual({ processed: 5, total: 10 });
    expect(result.current.target).toBe("a@b.com");
  });

  it("restores the sync's TRUE start time, not the remount instant", async () => {
    // The reported bug: leaving a tab mid-sync and coming back restarted the elapsed timer at 0:00,
    // because the bar had nothing but its own mount instant to count from. This mount is standing in
    // for that return — the stamp must come from the backend snapshot.
    const startedAt = 1_700_000_000_000;
    const { opts } = makeOpts({
      fetchStatus: vi.fn(async () =>
        idle({ running: true, processed: 5, total: 10, started_at_ms: startedAt }),
      ),
    });
    const { result } = renderHook(() => useDetachedSync(opts));
    await waitFor(() => expect(result.current.syncing).toBe(true));
    expect(result.current.startedAt).toBe(startedAt);
  });

  it("keeps the start time across per-file progress events", async () => {
    // The trap in the obvious implementation: folding the stamp into `progress` means the next
    // `counted` / `item` event replaces the object and drops it, restoring the bug one file later.
    const startedAt = 1_700_000_000_000;
    const { opts, emit } = makeOpts({
      fetchStatus: vi.fn(async () =>
        idle({ running: true, processed: 1, total: 9, started_at_ms: startedAt }),
      ),
    });
    const { result } = renderHook(() => useDetachedSync(opts));
    await waitFor(() => expect(result.current.startedAt).toBe(startedAt));

    await emit({ type: "counted", total: 9, target: null });
    expect(result.current.startedAt).toBe(startedAt);
    await emit({ type: "item", processed: 4, total: 9, name: "f.md" });
    expect(result.current.startedAt).toBe(startedAt);

    await emit({ type: "finished", report: REPORT });
    expect(result.current.startedAt).toBeNull(); // idle again
  });

  it("restores the last report when idle on mount", async () => {
    const { opts } = makeOpts({ fetchStatus: vi.fn(async () => idle({ last_report: REPORT })) });
    const { result } = renderHook(() => useDetachedSync(opts));
    await waitFor(() => expect(result.current.report).toEqual(REPORT));
    expect(result.current.syncing).toBe(false);
  });

  it("maps counted / item / finished progress events", async () => {
    const { opts, emit } = makeOpts();
    const { result } = renderHook(() => useDetachedSync(opts));
    await waitFor(() => expect(opts.subscribe).toHaveBeenCalled());

    await emit({ type: "counted", total: 8, target: null });
    expect(result.current.progress).toEqual({ processed: 0, total: 8 });

    await emit({ type: "item", processed: 3, total: 8, name: "f.md" });
    expect(result.current.progress).toEqual({ processed: 3, total: 8 });
    expect(result.current.syncing).toBe(true);

    await emit({ type: "finished", report: REPORT });
    expect(result.current.progress).toBeNull();
    expect(result.current.syncing).toBe(false);
    expect(result.current.report).toEqual(REPORT);
    expect(opts.onSettled).toHaveBeenCalledTimes(1);
  });

  it("starts a sync optimistically and calls start(target)", async () => {
    const { opts } = makeOpts();
    const { result } = renderHook(() => useDetachedSync(opts));
    await waitFor(() => expect(opts.subscribe).toHaveBeenCalled());

    await act(() => result.current.sync("a@b.com"));
    expect(opts.start).toHaveBeenCalledWith("a@b.com");
    expect(result.current.target).toBe("a@b.com");
    expect(result.current.progress).toEqual({ processed: 0, total: null });
    expect(result.current.syncing).toBe(true);
  });

  it("queues a second target mid-sync instead of hijacking the running bar", async () => {
    const { opts, emit } = makeOpts();
    const { result } = renderHook(() => useDetachedSync(opts));
    await waitFor(() => expect(opts.subscribe).toHaveBeenCalled());

    await act(() => result.current.sync("a@b.com")); // starts it
    await emit({ type: "item", processed: 1, total: 5, name: "x" }); // a sync is on screen
    await act(() => result.current.sync("c@d.com")); // mid-run → queued, not a new start
    expect(result.current.queued.has("c@d.com")).toBe(true);
    expect(result.current.target).toBe("a@b.com");
  });

  it("moves a queued target from Queued to Syncing when its own pass starts", async () => {
    // The reported bug: queue a second account mid-sync and its row said "Queued" for the rest of the
    // run. The backend now sweeps each queued target in its own pass and announces it on `counted`.
    const { opts, emit } = makeOpts();
    const { result } = renderHook(() => useDetachedSync(opts));
    await waitFor(() => expect(opts.subscribe).toHaveBeenCalled());

    await act(() => result.current.sync("a@b.com"));
    await emit({ type: "counted", total: 5, target: "a@b.com" });
    await act(() => result.current.sync("c@d.com"));
    expect(result.current.queued.has("c@d.com")).toBe(true);

    // The first account's pass ends and the queued one's begins — same run, no `finished` yet.
    await emit({ type: "counted", total: 2, target: "c@d.com" });
    expect(result.current.target).toBe("c@d.com");
    expect(result.current.queued.has("c@d.com")).toBe(false);
    expect(result.current.syncing).toBe(true);
  });

  it("clears every queued row when a pass sweeps all targets", async () => {
    // An all-targets request subsumes the queued ones backend-side (one sweep covers them), so no row
    // may be left claiming it is still waiting for a pass that will never come.
    const { opts, emit } = makeOpts();
    const { result } = renderHook(() => useDetachedSync(opts));
    await waitFor(() => expect(opts.subscribe).toHaveBeenCalled());

    await act(() => result.current.sync("a@b.com"));
    await emit({ type: "counted", total: 5, target: "a@b.com" });
    await act(() => result.current.sync("c@d.com"));
    await act(() => result.current.sync("e@f.com"));
    expect(result.current.queued.size).toBe(2);

    await emit({ type: "counted", total: 4, target: null });
    expect(result.current.target).toBeNull();
    expect(result.current.queued.size).toBe(0);
  });

  it("rolls back the optimistic bar when start() rejects", async () => {
    const { opts } = makeOpts({
      start: vi.fn(async () => {
        throw new Error("boom");
      }),
    });
    const { result } = renderHook(() => useDetachedSync(opts));
    await waitFor(() => expect(opts.subscribe).toHaveBeenCalled());

    await act(() => result.current.sync("a@b.com"));
    await waitFor(() => expect(result.current.error).toBe("Error: boom"));
    expect(result.current.progress).toBeNull();
    expect(result.current.target).toBeNull();
  });

  it("run() toggles busy and surfaces a thrown error", async () => {
    const { opts } = makeOpts();
    const { result } = renderHook(() => useDetachedSync(opts));
    await waitFor(() => expect(opts.subscribe).toHaveBeenCalled());

    await act(async () => {
      await result.current.run("connect", async () => {
        throw new Error("nope");
      });
    });
    expect(result.current.error).toBe("Error: nope");
    expect(result.current.busy).toBeNull();
  });

  it("requestStop sets stopping and calls stop()", async () => {
    const { opts } = makeOpts();
    const { result } = renderHook(() => useDetachedSync(opts));
    await waitFor(() => expect(opts.subscribe).toHaveBeenCalled());

    await act(() => result.current.requestStop());
    expect(result.current.stopping).toBe(true);
    expect(opts.stop).toHaveBeenCalledTimes(1);
  });

  it("unsubscribes on unmount", async () => {
    const { opts, unlisten } = makeOpts();
    const { unmount } = renderHook(() => useDetachedSync(opts));
    await waitFor(() => expect(opts.subscribe).toHaveBeenCalled());
    unmount();
    await waitFor(() => expect(unlisten).toHaveBeenCalledTimes(1));
  });
});
