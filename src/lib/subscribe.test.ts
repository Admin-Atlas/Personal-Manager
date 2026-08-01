// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later
// @vitest-environment jsdom
//
// The whole point of the helper is an ordering nobody can see by reading a component: the unlisten
// handle arriving AFTER the effect that asked for it was torn down. Pinned here because a leaked
// listener is silent — it costs a duplicate handler per mount and nothing ever reports it.
//
// jsdom rather than the default node environment for one case only: the StrictMode check at the
// bottom is the actual shipped hazard, and it needs React to double-invoke a real effect.

import type { UnlistenFn } from "@tauri-apps/api/event";
import { renderHook } from "@testing-library/react";
import { StrictMode, useEffect } from "react";
import { describe, expect, it, vi } from "vitest";
import { subscribeUntilCleanup } from "./subscribe";

/** A subscribe() whose promise only settles when the test says so — the real ordering hazard. */
function deferred() {
  let resolve: (un: UnlistenFn) => void = () => {};
  let reject: (e: unknown) => void = () => {};
  const promise = new Promise<UnlistenFn>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { subscribe: () => promise, resolve, reject };
}

/** Let the helper's `.then` / `.catch` run. */
const settle = () => new Promise((r) => setTimeout(r, 0));

/** Node's unhandled-rejection hook. `process` is not in `src/`'s DOM-only lib set (this is the only
 *  test that wants it), so it is reached through globalThis with just the two methods needed. */
const { process: node } = globalThis as unknown as {
  process: {
    on(event: "unhandledRejection", fn: (reason: unknown) => void): void;
    off(event: "unhandledRejection", fn: (reason: unknown) => void): void;
  };
};

describe("subscribeUntilCleanup", () => {
  it("unsubscribes a handle that arrives after cleanup has already run", async () => {
    // THE regression: the hand-rolled version writes this handle into a closure nobody will call
    // again, and the listener stays live for the rest of the session.
    const unlisten = vi.fn();
    const { subscribe, resolve } = deferred();
    const off = subscribeUntilCleanup(subscribe);

    off();
    resolve(unlisten);
    await settle();

    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("unsubscribes once when the handle arrives first, however often cleanup is called", async () => {
    const unlisten = vi.fn();
    const { subscribe, resolve } = deferred();
    const off = subscribeUntilCleanup(subscribe);

    resolve(unlisten);
    await settle();
    expect(unlisten).not.toHaveBeenCalled(); // still subscribed — cleanup hasn't run

    off();
    off();
    expect(unlisten).toHaveBeenCalledTimes(1);
  });

  it("swallows a failed subscribe instead of raising an unhandled rejection", async () => {
    // These are fire-and-forget event wirings inside effects; there is nothing a caller could do,
    // and an unhandled rejection out of a cleanup path is noise a reader would chase.
    const seen: unknown[] = [];
    const record = (reason: unknown) => seen.push(reason);
    node.on("unhandledRejection", record);
    try {
      const { subscribe, reject } = deferred();
      const off = subscribeUntilCleanup(subscribe);
      reject(new Error("the event channel is gone"));
      await settle();

      expect(() => off()).not.toThrow();
      await settle();
      expect(seen).toEqual([]);
    } finally {
      node.off("unhandledRejection", record);
    }
  });

  it("settles at one live listener through StrictMode's double-invoked effect", async () => {
    // The shipped symptom, end to end: dev mounts every effect twice, so the naive version leaves
    // the first subscription live for ever and every event is handled twice.
    const { subscribe, register, live } = leakCounter();
    const { unmount } = renderHook(() => useSubscription(subscribe), { wrapper: StrictMode });

    expect(subscribe).toHaveBeenCalledTimes(2); // mount → cleanup → mount
    register();
    await settle();
    expect(live()).toBe(1);

    unmount();
    await settle();
    expect(live()).toBe(0);
  });
});

function useSubscription(subscribe: () => Promise<UnlistenFn>) {
  useEffect(() => subscribeUntilCleanup(subscribe), [subscribe]);
}

/** A fake backend that counts registered listeners, and only registers them when asked — so the
 *  subscribe promises are still in flight when StrictMode tears the first effect down. */
function leakCounter() {
  let live = 0;
  const waiting: (() => void)[] = [];
  const subscribe = vi.fn(
    () =>
      new Promise<UnlistenFn>((resolve) => {
        waiting.push(() => {
          live += 1;
          resolve(() => {
            live -= 1;
          });
        });
      }),
  );
  return {
    subscribe,
    /** Register every outstanding subscription, as the backend eventually would. */
    register: () => waiting.splice(0).forEach((go) => go()),
    live: () => live,
  };
}
