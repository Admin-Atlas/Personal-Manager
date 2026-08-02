// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The React half of `wheelShift.ts`: attach a horizontal-swipe day-stepper to an element.
//
// Deliberately a manual `addEventListener` rather than an `onWheel` prop, because the listener must
// be NON-PASSIVE to call preventDefault — React attaches wheel listeners passively, where
// preventDefault is ignored (with a console warning). Without it, a horizontal trackpad swipe can
// also trigger the browser's back-navigation gesture, and the day step would come with a page
// change.
//
// IT RETURNS A CALLBACK REF, NOT A REF OBJECT, and that is the whole reliability story. Binding
// inside an effect keyed on `[ref, enabled]` looks equivalent and is not: the effect fires once, on
// mount, and `ref.current` is only populated if the target happens to be in the DOM at that instant.
// The Calendar tab's grid is behind `hasCalendars || hasOverlay`, both false until the overview
// resolves, so on a cold open the effect ran against `null` and — with neither dep ever changing —
// never ran again. The gesture was dead for the rest of the session. The same shape bit us from the
// other side when the target was a `key`ed node: React remounts it, the old listener goes with the
// detached node and nothing rebinds. React calls a callback ref with the node on every mount and
// with `null` on every unmount, which is exactly the lifecycle a raw listener needs, so both failure
// modes go away and callers stop having to know which of their divs is safe to point it at.

// The callback RETURNS ITS CLEANUP, which React 19 supports and which is the only race-free shape.
// Storing the detach in a shared ref and running it on the next call assumes React always detaches
// the old node before attaching the new one; when a commit does it the other way round, the shared
// slot holds the NEW detach by the time the old node's call runs, and the listener that was just
// attached is torn straight off again — the element is live, looks bound, and never fires. Returning
// the cleanup ties it to the attachment it belongs to and React never passes `null` at all.
import { useCallback, useRef } from "react";
import { accumulateShift, horizontalPixels, isHorizontalGesture } from "./wheelShift";

/**
 * Step `onShift(±n)` as the user swipes sideways over the element this ref is attached to.
 *
 * `onShift` and `enabled` are held in refs so a caller can pass a fresh closure each render (the
 * normal case — it closes over the current cursor) and can flip `enabled` without the ref identity
 * changing, which would make React detach and re-attach the node's listener mid-gesture.
 */
export function useHorizontalWheelShift(
  onShift: (days: number) => void,
  enabled = true,
): (el: HTMLElement | null) => (() => void) | undefined {
  const shiftRef = useRef(onShift);
  shiftRef.current = onShift;
  const enabledRef = useRef(enabled);
  enabledRef.current = enabled;

  return useCallback((el: HTMLElement | null) => {
    if (!el) return;
    // Per-attachment, so a remount starts a fresh gesture rather than inheriting stale travel.
    let carry = 0;
    const onWheel = (e: WheelEvent) => {
      if (!enabledRef.current || !isHorizontalGesture(e)) return;
      // Claim the gesture even when it hasn't yet accumulated a whole step: the alternative is that
      // the first two-thirds of every swipe navigates the app backwards.
      e.preventDefault();
      const { steps, carry: next } = accumulateShift(carry, horizontalPixels(e));
      carry = next;
      if (steps !== 0) shiftRef.current(steps);
    };
    // CAPTURE, so nothing rendered inside the day window can consume the gesture first. The grids
    // below are rebuilt whenever the cursor moves, and a descendant that claims one wheel event is
    // otherwise enough to make the swipe work once and then appear to die.
    el.addEventListener("wheel", onWheel, { passive: false, capture: true });
    return () => el.removeEventListener("wheel", onWheel, { capture: true });
  }, []);
}
