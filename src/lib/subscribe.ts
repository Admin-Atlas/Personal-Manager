// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// One helper for the shape every Tauri event subscription in an effect has to get right: `listen()`
// is async, but an effect's cleanup is not, so the unlisten handle can arrive AFTER the effect that
// asked for it has already been torn down.
//
// Written out by hand, the wrong version reads perfectly:
//
//   let off: UnlistenFn | undefined;
//   void listen(handler).then((un) => (off = un));
//   return () => off?.();
//
// — and leaks. The cleanup runs while `off` is still undefined, the promise resolves a tick later
// into a closure nobody will call again, and the listener stays live for the rest of the session.
// StrictMode's double-invoked effects make that the NORMAL ordering in dev, so the listener count
// climbs by one per mount and every event is handled twice.
//
// Sites that need this are scattered across App/TitleBar/hooks, so the rule lives here rather than
// being re-derived (and re-fumbled) at each one — and, decisively, here it is testable: a pure
// `src/lib` helper has a test file where a component effect does not.

import type { UnlistenFn } from "@tauri-apps/api/event";

/**
 * Subscribe for one effect's lifetime, and return that effect's cleanup.
 *
 * If teardown happens before the subscribe promise resolves, the handle is called the moment it
 * arrives instead of being written into a dead closure — so the listener never outlives the effect.
 * Idempotent: calling the returned cleanup twice unsubscribes once.
 *
 * A rejecting subscribe is swallowed. These are all fire-and-forget event wirings whose failure the
 * caller could do nothing about, and an unhandled rejection out of a cleanup path would be noise.
 */
export function subscribeUntilCleanup(subscribe: () => Promise<UnlistenFn>): () => void {
  let unlisten: UnlistenFn | undefined;
  let cancelled = false;
  void subscribe()
    .then((un) => {
      if (cancelled) un();
      else unlisten = un;
    })
    .catch(() => {});
  return () => {
    cancelled = true;
    unlisten?.();
    unlisten = undefined;
  };
}
