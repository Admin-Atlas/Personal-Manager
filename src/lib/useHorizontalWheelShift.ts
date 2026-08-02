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
// THE LISTENER LIVES ON `window`, AND THE REGION IS A HIT TEST — not the other way round. Three
// shapes were tried on the element itself and each died a different death:
//
//   * an effect keyed on `[ref, enabled]` binds once, on mount, and `ref.current` is only populated
//     if the target is in the DOM at that instant. The Calendar's grid renders behind
//     `hasCalendars || hasOverlay`, both false until the overview resolves, so the effect ran
//     against `null` and — with neither dep ever changing — never ran again;
//   * a callback ref fixes that, but binds to whatever node React hands over, so a `key`ed remount
//     silently takes the listener with the detached node;
//   * a callback ref that RETURNS its cleanup (the React 19 form) fixes the remount, and still
//     leaves the real one: ownership by DOM ancestry needs the event to keep being dispatched
//     through a node inside the region. The calendar's day columns are keyed by date, so all of
//     them are replaced the moment a step lands, and the rest of that gesture is targeted at nodes
//     that are no longer in the tree. One day, then silence — the reported symptom, twice.
//
// A window listener installed for the hook's whole lifetime has none of those failure modes: it
// exists before the region does, it survives every remount underneath it, and it never needs to
// know which of the caller's divs is safe to point at. `withinRect` then decides ownership from the
// pointer's coordinates, which no re-render can invalidate. `el.contains(target)` is kept as the
// FIRST test purely because it is free — `getBoundingClientRect` forces a layout read, and the
// fallback is only reached when the target really has left the tree.
//
// This is the same shape `scrollAxis.ts` uses app-wide (window + capture + non-passive), which is
// also why the two coexist: neither stops propagation, and over a day grid the normaliser finds no
// horizontal scroller and leaves the event alone.
import { useCallback, useEffect, useRef } from "react";
import { accumulateShift, horizontalPixels, isHorizontalGesture, withinRect } from "./wheelShift";

/**
 * Step `onShift(±n)` as the user swipes sideways over the element this ref is attached to.
 *
 * `onShift` and `enabled` are held in refs so a caller can pass a fresh closure each render (the
 * normal case — it closes over the current cursor) and can flip `enabled` without re-binding.
 */
export function useHorizontalWheelShift(
  onShift: (days: number) => void,
  enabled = true,
): (el: HTMLElement | null) => void {
  const shiftRef = useRef(onShift);
  shiftRef.current = onShift;
  const enabledRef = useRef(enabled);
  enabledRef.current = enabled;
  // Read live inside the listener, never closed over: the region may mount long after the listener,
  // and may be swapped for a different node any number of times while it is installed.
  const elRef = useRef<HTMLElement | null>(null);

  useEffect(() => {
    let carry = 0;
    const onWheel = (e: WheelEvent) => {
      const el = elRef.current;
      if (!el || !enabledRef.current || !isHorizontalGesture(e)) return;
      const target = e.target;
      const owned =
        (target instanceof Node && el.contains(target)) ||
        withinRect(el.getBoundingClientRect(), e.clientX, e.clientY);
      if (!owned) {
        // A sideways gesture somewhere else entirely: drop the accumulated travel rather than let
        // it fire on the next, unrelated swipe over the grid.
        carry = 0;
        return;
      }
      // Claim the gesture even when it hasn't yet accumulated a whole step: the alternative is that
      // the first two-thirds of every swipe navigates the app backwards.
      e.preventDefault();
      const { steps, carry: next } = accumulateShift(carry, horizontalPixels(e));
      carry = next;
      if (steps !== 0) shiftRef.current(steps);
    };
    // CAPTURE, so nothing between the window and the pointer can consume the gesture first.
    window.addEventListener("wheel", onWheel, { passive: false, capture: true });
    return () => window.removeEventListener("wheel", onWheel, { capture: true });
  }, []);

  return useCallback((el: HTMLElement | null) => {
    elRef.current = el;
  }, []);
}
