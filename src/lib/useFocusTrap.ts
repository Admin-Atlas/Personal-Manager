// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Keep keyboard focus inside a container while it's active (a modal dialog): move focus in on open,
// wrap Tab/Shift+Tab at the edges, and hand focus back to whatever opened it on close. Extends the
// save/restore pattern in ui/Popover.tsx with the trap Popover deliberately omits. Escape stays the
// caller's job (Modal already handles it).
//
// The opener is captured during the render that first sees `active` — BEFORE React commits the
// dialog and applies any child `autoFocus`, which would otherwise move `document.activeElement`
// inside the dialog and make "restore" point at an element that's about to unmount.
//
// Pure DOM + refs, so it unit-tests in jsdom: jsdom doesn't move focus on a real Tab keypress, but
// it does honour programmatic `.focus()` and dispatched keydown events, which is all this relies on.
//
// NESTED DIALOGS: a trap stands down while a deeper modal dialog holds focus. `Modal` does not
// portal, so a dialog opened from inside another one is a DOM DESCENDANT of it and the same
// bubbling keydown reaches both traps. Without this rule the outer trap also runs and wraps Tab
// against the union of BOTH dialogs' focusables — so Tab off the last button of a confirmation
// lands somewhere in the window behind it. See lib/useDialogLayer.ts for the Escape half of the
// same problem.

import { useEffect, useRef, type RefObject } from "react";

/** A modal dialog nested inside `container` currently holds focus, so this trap is not the one in
 *  charge. Matches on the ARIA that makes a dialog modal rather than on a component, so a
 *  hand-rolled overlay that declares the same semantics is honoured too. */
function deeperDialogHasFocus(container: HTMLElement): boolean {
  const nested = container.querySelectorAll<HTMLElement>('[role="dialog"][aria-modal="true"]');
  return Array.from(nested).some((el) => el.contains(document.activeElement));
}

const FOCUSABLE = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  '[tabindex]:not([tabindex="-1"])',
].join(",");

function focusable(container: HTMLElement): HTMLElement[] {
  return Array.from(container.querySelectorAll<HTMLElement>(FOCUSABLE));
}

export function useFocusTrap(active: boolean, containerRef: RefObject<HTMLElement | null>): void {
  // Capture the opener during render, before the dialog's child autoFocus can steal it (see header).
  const openerRef = useRef<HTMLElement | null>(null);
  const wasActive = useRef(false);
  if (active && !wasActive.current) {
    openerRef.current = document.activeElement as HTMLElement | null;
  }
  wasActive.current = active;

  useEffect(() => {
    if (!active) return;
    const container = containerRef.current;
    if (!container) return;

    // Initial focus — unless a child already grabbed it (React `autoFocus`): honour that.
    if (!container.contains(document.activeElement)) {
      const initial =
        container.querySelector<HTMLElement>("[data-autofocus]") ??
        focusable(container)[0] ??
        container;
      initial.focus();
    }

    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Tab") return;
      if (deeperDialogHasFocus(container)) return;
      const items = focusable(container);
      if (items.length === 0) {
        e.preventDefault();
        container.focus();
        return;
      }
      const first = items[0];
      const last = items[items.length - 1];
      const activeEl = document.activeElement;
      if (e.shiftKey) {
        if (activeEl === first || !container.contains(activeEl)) {
          e.preventDefault();
          last.focus();
        }
      } else if (activeEl === last || !container.contains(activeEl)) {
        e.preventDefault();
        first.focus();
      }
    };

    container.addEventListener("keydown", onKey);
    return () => {
      container.removeEventListener("keydown", onKey);
      const opener = openerRef.current;
      if (opener && document.contains(opener)) opener.focus();
    };
  }, [active, containerRef]);
}
