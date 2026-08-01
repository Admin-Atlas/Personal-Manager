// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Which open dialog owns the keyboard. Until now exactly one dialog was ever open at a time, so
// `Modal` could bind Escape to `window` and be right by luck. Settings is about to become a Modal
// (the hand-rolled-overlay fix), and Settings contains dialogs: its own unsaved-changes guard, the
// per-tab reset confirmations, the re-index progress, the storage confirmations, and the three
// remove-my-data steps. With every Modal listening on `window`, ONE Escape would fire the guard's
// `onClose` AND the outer `requestClose` — driving straight through the unsaved-edit guard that
// `requestClose` exists to enforce. That is a behaviour regression in a save guard, so the rule has
// to land before Settings nests, not with it.
//
// THE RULE: a dialog is "topmost" when no other open dialog is nested inside it.
//
// Deliberately DOM containment, not a push/pop stack keyed on registration order. React commits
// child effects BEFORE parent effects, so two dialogs opening in the same commit register
// inner-first and an order-based stack names the wrong winner. Containment is the same truth
// without the ordering hazard — and it is only truth at all because `Modal` does not portal, so a
// nested dialog really is a DOM descendant of the outer one. If Modal ever gains a portal, this
// file has to change with it.
//
// Two dialogs open side by side (neither inside the other) are both topmost, which is exactly
// today's behaviour — this rule fixes nesting and deliberately regresses nothing else.

import { useCallback, useEffect, type RefObject } from "react";

/** Every open dialog's container element. Module-level: the question "is anything deeper than me
 *  open?" is about the whole app, not about one React subtree. */
const openDialogs = new Set<HTMLElement>();

/**
 * Register a dialog while it is open, and get back a predicate for "am I the one the keyboard
 * should be talking to?". Read the predicate at EVENT time, never at render time — the set changes
 * whenever any dialog opens or closes, and a value captured during render is stale by then.
 */
export function useDialogLayer(
  active: boolean,
  containerRef: RefObject<HTMLElement | null>,
): () => boolean {
  useEffect(() => {
    if (!active) return;
    const el = containerRef.current;
    if (!el) return;
    openDialogs.add(el);
    return () => {
      openDialogs.delete(el);
    };
  }, [active, containerRef]);

  return useCallback(() => {
    const el = containerRef.current;
    // Not mounted yet: nothing can be nested inside it, so it cannot be shadowed by anything.
    if (!el) return true;
    for (const other of openDialogs) {
      if (other !== el && el.contains(other)) return false;
    }
    return true;
  }, [containerRef]);
}
