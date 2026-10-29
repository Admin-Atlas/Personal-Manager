// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The browser sign-ins PM is waiting on (`signin://open` / `signin://closed`), for `SignInStrip`.
//
// Module-level rather than a React context, like `documentFeed`: the strip is drawn in two places
// (inside Settings while it's open, at the top of the app otherwise), each mounting and unmounting
// with Settings, while the sign-in events must be followed from before the first one can start.
// App follows them for the life of the window (`useFollowSignIns`); the strips only read.

import { useEffect, useSyncExternalStore } from "react";

import { onSignInClosed, onSignInOpened } from "./ipc";
import { subscribeUntilCleanup } from "./subscribe";
import type { SignInPage } from "./types";

let pages: SignInPage[] = [];
const listeners = new Set<() => void>();

function set(next: SignInPage[]) {
  pages = next;
  for (const l of listeners) l();
}

/** A sign-in shown, or the same one told something new (its browser wouldn't open). */
export function signInOpened(page: SignInPage) {
  set(
    pages.some((p) => p.id === page.id)
      ? pages.map((p) => (p.id === page.id ? page : p))
      : [...pages, page],
  );
}

/** A sign-in's wait ended, however it ended. */
export function signInClosed(id: number) {
  if (pages.some((p) => p.id === id)) set(pages.filter((p) => p.id !== id));
}

/** Follow the backend's sign-ins while mounted. Mount once, where it lives as long as the window. */
export function useFollowSignIns(): void {
  useEffect(() => subscribeUntilCleanup(() => onSignInOpened(signInOpened)), []);
  useEffect(() => subscribeUntilCleanup(() => onSignInClosed(signInClosed)), []);
}

function subscribe(listener: () => void) {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

/** The sign-ins waiting, oldest first. */
export function useSignInPages(): SignInPage[] {
  return useSyncExternalStore(subscribe, () => pages);
}
