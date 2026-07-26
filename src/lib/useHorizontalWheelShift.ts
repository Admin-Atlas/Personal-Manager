// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The React half of `wheelShift.ts`: attach a horizontal-swipe day-stepper to an element.
//
// Deliberately a manual `addEventListener` rather than an `onWheel` prop, because the listener must
// be NON-PASSIVE to call preventDefault — React attaches wheel listeners passively, where
// preventDefault is ignored (with a console warning). Without it, a horizontal trackpad swipe can
// also trigger the browser's back-navigation gesture, and the day step would come with a page
// change.

import { useEffect, useRef, type RefObject } from "react";
import { accumulateShift, isHorizontalGesture } from "./wheelShift";

/**
 * Step `onShift(±n)` as the user swipes sideways over `ref`.
 *
 * `onShift` is held in a ref so a caller can pass a fresh closure each render (the normal case — it
 * closes over the current cursor) without re-subscribing the listener and dropping the accumulated
 * carry mid-gesture.
 */
export function useHorizontalWheelShift(
  ref: RefObject<HTMLElement | null>,
  onShift: (days: number) => void,
  enabled = true,
): void {
  const shiftRef = useRef(onShift);
  shiftRef.current = onShift;

  useEffect(() => {
    const el = ref.current;
    if (!el || !enabled) return;
    let carry = 0;
    const onWheel = (e: WheelEvent) => {
      if (!isHorizontalGesture(e)) return;
      // Claim the gesture even when it hasn't yet accumulated a whole step: the alternative is that
      // the first two-thirds of every swipe navigates the app backwards.
      e.preventDefault();
      const { steps, carry: next } = accumulateShift(carry, e.deltaX);
      carry = next;
      if (steps !== 0) shiftRef.current(steps);
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [ref, enabled]);
}
