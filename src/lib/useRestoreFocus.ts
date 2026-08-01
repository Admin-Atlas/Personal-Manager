// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Remember which element had focus when a floating panel opened, and hand focus back to it on
// demand. Extracted verbatim in behaviour from `ui/Popover.tsx`, which has done this since the
// calendar chrome landed, so the other non-modal panels can have it without a second copy — most
// immediately `calendar/parts/CalendarEventPopover.tsx`, a singleton driven from selection state
// that cannot use Popover (Popover owns its trigger through a render prop; the calendar's panel is
// anchored to a `DOMRect` handed up by any of dozens of chips).
//
// WHY `restore()` IS IMPERATIVE and not automatic on close. An outside click has ALREADY moved
// focus to whatever was clicked, so restoring would yank it straight back off the thing the user
// just aimed at. Only the dismissals that leave focus nowhere — Escape, a Close button — should
// restore. `useFocusTrap` is the modal counterpart and restores unconditionally, which is right
// there for the opposite reason: a modal's scrim means no outside click can land on anything.
//
// The opener is captured during the render that first sees `active` — BEFORE React commits the
// panel and any child `autoFocus` runs, which would otherwise make "the opener" an element inside
// the panel that is about to unmount. Same reasoning, and same shape, as `useFocusTrap`'s header.
// Capturing on the false→true EDGE also fixes a latent bug in the version this replaces: Popover
// re-captured inside an effect whose deps included a callback identity, so a parent re-render while
// the panel was open could quietly re-point "the opener" at something inside the panel.
//
// `openerKey` is the second, rarer edge: a SINGLETON panel that is re-pointed at a new opener
// without closing first. `CalendarEventPopover` is one — activating another event chip from the
// KEYBOARD swaps `event`/`anchor` on the mounted instance (the mouse path unmounts it first, via the
// outside-mousedown dismissal, so only the keyboard reaches this). Without the key, "the opener"
// stays the first chip the user ever clicked, and Escape hands focus back to an event they left
// several chips ago. The default `undefined` never changes, so a caller that doesn't pass one —
// Popover — keeps exactly the mount-edge behaviour above.

import { useCallback, useRef } from "react";

/**
 * @param active whether the panel is open.
 * @param openerKey identifies WHICH opener the panel currently belongs to. Re-captures whenever it
 *        changes while open, compared by `Object.is`. Omit for a panel that closes between openers.
 * @returns `restore()` — focus the element that was focused when the panel opened, if it is still
 *          in the document. Safe to call more than once, and a no-op when nothing was captured.
 */
export function useRestoreFocus(active: boolean, openerKey?: unknown): () => void {
  const openerRef = useRef<HTMLElement | null>(null);
  const wasActive = useRef(false);
  // Seeded WITH the first key, so the mount capture below is the `!wasActive` one and a panel that
  // opens already keyed doesn't capture twice.
  const lastKey = useRef(openerKey);
  const reKeyed = !Object.is(lastKey.current, openerKey);
  if (active && (!wasActive.current || reKeyed)) {
    openerRef.current = document.activeElement as HTMLElement | null;
  }
  wasActive.current = active;
  lastKey.current = openerKey;

  return useCallback(() => {
    const opener = openerRef.current;
    if (opener && document.contains(opener)) opener.focus();
  }, []);
}
