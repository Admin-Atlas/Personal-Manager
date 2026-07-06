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

    await emit({ type: "counted", total: 8 });
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
