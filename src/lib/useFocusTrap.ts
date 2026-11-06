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
//
// WHEN THE OPENER IS GONE. Restoring needs the opener still in the page, and it can go while the
// dialog is up: the row that opened it re-renders away, or it was the link notice's ×, which moves
// into the topmost dialog as a new element (LinkNotice). An opener that was the body (blurred before
// the dialog opened) counts as none. If focus fell with the dialog, it then goes to the dialog this
// one was opened over, if any, so it stays inside that dialog's trap instead of on the body outside
// it; focus something has been given since (the event editor's Save, from a timer) is left alone.
// And an opener marked `data-focus-return="<name>"` is handed on: the notice's new × mounts a
// commit after this cleanup (once useDialogLayer's change re-renders it), claims the name from its
// layout effect (`claimFocusReturn`), and takes focus if focus is still where this left it. React
// runs that commit before the next microtask, when an unclaimed hand-on lapses.

import { useEffect, useRef, type RefObject } from "react";

/** An opener's mark: the name of the element that stands in for it once it's gone. */
const FOCUS_RETURN = "data-focus-return";

/** The one hand-on waiting to be claimed: the gone opener's name, and the element this trap's
 *  cleanup left focus on (null: the body, focus on nothing). */
let handedOn: { name: string; left: Element | null } | null = null;

/** For an element marked `data-focus-return={name}`, from its layout effect when it mounts: take
 *  focus if a trap just closed whose opener was the element of that name, now gone, and nothing has
 *  moved focus since. */
export function claimFocusReturn(name: string, el: HTMLElement): void {
  const waiting = handedOn;
  if (waiting === null || waiting.name !== name) return;
  handedOn = null;
  const at = document.activeElement;
  const untouched = waiting.left ? at === waiting.left : at === null || at === document.body;
  if (untouched && el.isConnected) el.focus();
}

/** A modal dialog nested inside `container` currently holds focus, so this trap is not the one in
 *  charge. Matches on the ARIA that makes a dialog modal rather than on a component, so a
 *  hand-rolled overlay that declares the same semantics is honoured too. */
function deeperDialogHasFocus(container: HTMLElement): boolean {
  const nested = container.querySelectorAll<HTMLElement>('[role="dialog"][aria-modal="true"]');
  return Array.from(nested).some((el) => el.contains(document.activeElement));
}

// TAB-REACHABLE, not merely focusable. `tabindex="-1"` means "focusable by script, not by Tab", so
// an element carrying it is never a stop the trap should wrap against — and the generic arm below
// always said so while the five element arms did not. The command palette is where that inconsistency
// bites: its result rows are `<button tabIndex={-1}>` driven by `aria-activedescendant`, so the trap
// counted them as stops, decided the input was neither first nor last, and let the very first Tab
// walk straight out of the dialog. With them excluded the input is the only stop and Tab refocuses
// it, which is what a palette should do.
const FOCUSABLE = [
  'a[href]:not([tabindex="-1"])',
  'button:not([disabled]):not([tabindex="-1"])',
  'input:not([disabled]):not([tabindex="-1"])',
  'select:not([disabled]):not([tabindex="-1"])',
  'textarea:not([disabled]):not([tabindex="-1"])',
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
    // The dialog this one was opened over (Modal doesn't portal, so it's an ancestor), where focus
    // goes back to if the opener is gone by the time this one closes (see header).
    const outer =
      container.parentElement?.closest<HTMLElement>('[role="dialog"][aria-modal="true"]') ?? null;

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
        // The container itself counts as the top: it holds focus after a click on the dialog's
        // background (`tabIndex={-1}` takes a click) or when the link notice's × hands it back
        // (LinkNotice), and the engine's own Shift+Tab from there walks out of the dialog.
        if (activeEl === first || activeEl === container || !container.contains(activeEl)) {
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
      if (opener && opener !== document.body && document.contains(opener)) {
        opener.focus();
        return;
      }
      // Gone (or never anything). Focus someone has put somewhere since (the event editor's Save,
      // from a timer) stays there; focus that fell with this dialog goes to the dialog underneath.
      const at = document.activeElement;
      if (at !== null && at !== document.body) return;
      let left: Element | null = null;
      if (outer && document.contains(outer)) {
        outer.focus();
        left = outer;
      }
      const name = opener?.getAttribute(FOCUS_RETURN);
      if (name) {
        const waiting = { name, left };
        handedOn = waiting;
        queueMicrotask(() => {
          if (handedOn === waiting) handedOn = null;
        });
      }
    };
  }, [active, containerRef]);
}
