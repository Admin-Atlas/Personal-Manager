// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

/** The one-way "PM is being erased — stop writing things down" signal.
 *
 *  "Remove PM data" clears the webview's own storage before the backend deletes the OS-level folder
 *  behind it, so a late flush writes nothing. That only holds if nothing writes to it AFTER the
 *  clear — and two things did. The theme provider re-persists eleven keys whenever an axis or the
 *  resolved mode changes, and its driver listens to `matchMedia("(prefers-color-scheme: dark)")` and
 *  `visibilitychange`, so simply switching away from the window could put them all back. The two
 *  15-minute pollers, meanwhile, kept asking a machine that had just been wiped.
 *
 *  Deliberately a module-level flag rather than React state: the writers are inside effects and
 *  event handlers that must be able to check it synchronously, mid-callback, without a re-render.
 *  One-way for the life of the page — the app quits or reloads straight after, and re-arming it
 *  would only ever be a way to reintroduce the bug.
 *
 *  Scoped to ONE webview. The briefing window is a separate JS context writing to the same origin
 *  store, so it cannot see this flag; it is destroyed outright instead (see `RemovePmData`). */

let tearingDown = false;
const listeners = new Set<() => void>();

/** Whether the erase has started. Cheap enough to call on every write. */
export function isTearingDown(): boolean {
  return tearingDown;
}

/** Start the teardown: no further persistence, and every subscriber stands down. Idempotent, so a
 *  retried or re-entered wipe costs nothing. */
export function beginTeardown(): void {
  if (tearingDown) return;
  tearingDown = true;
  for (const listener of listeners) {
    try {
      listener();
    } catch {
      /* one bad subscriber must not stop the others standing down */
    }
  }
}

/** Run `listener` when the teardown starts — or immediately, if it already has, so a component that
 *  mounts late can't miss the signal and keep polling. Returns an unsubscribe for cleanup. */
export function onTeardown(listener: () => void): () => void {
  if (tearingDown) {
    listener();
    return () => {};
  }
  listeners.add(listener);
  return () => listeners.delete(listener);
}

/** Test-only reset. Not exported for app code: the flag is one-way by design, and the only reason
 *  to clear it is that vitest shares one module instance across the cases in a file. */
export function resetTeardownForTests(): void {
  tearingDown = false;
  listeners.clear();
}
