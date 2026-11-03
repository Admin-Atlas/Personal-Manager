// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Runway's pointer drag (PA-2): pick up a task card (or a closed stack, as one) and drop it in
// another column's lane or on the parked shelf. Hand-rolled on pointer events, no HTML5 drag and
// drop (Tauri's native file-drop handler swallows in-page HTML5 drags on Windows), in the shape of
// `lib/pinboard/useBoardDrag.ts`:
//
//   - the listeners go on `window` for the gesture's life, so it survives the pointer leaving the
//     card or the board;
//   - where it is over is decided by GEOMETRY, every move: each drop zone carries
//     `data-runway-zone` (and `data-runway-col` + `data-runway-lane`, or `data-runway-parked`),
//     and its box is measured with `getBoundingClientRect`, never by which element the event hit
//     (the #698 lesson: a dragged card's own subtree is what the pointer is over most of the time);
//   - nothing starts until the pointer has moved 4px, so a click on a card still clicks;
//   - Escape, pointercancel and the window losing focus abandon it, with nothing written.
//
// The drop itself is the caller's: this only says what was dropped, on which zone, and before
// which card. Cards carry `data-runway-card="<zone key>"`; the card being dragged is skipped when
// counting the place, so the index is among the cards that stay.
//
// The time grid's own gesture (`useCalendarGesture`) is gated off outside Day/Week/Month, so the two
// never meet.

import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from "react";

/** What's being dragged: one task, or a stack's tasks together. */
export interface RunwayDragItem {
  /** The card's own key (`task:<id>` / `stack:<id>:<column>:<lane>`), to dim it while it moves. */
  key: string;
  taskIds: number[];
}

/** Where a drag is over: a zone, and the card it would land before (among those that stay). */
export interface RunwayDropAt {
  zone: string;
  target:
    { kind: "column"; columnId: string; lane: "foreground" | "background" } | { kind: "parked" };
  index: number;
}

export interface RunwayDragOptions {
  /** The board's root: only zones inside it are hit-tested. */
  rootRef: { current: HTMLElement | null };
  onDrop: (item: RunwayDragItem, at: RunwayDropAt) => void;
  /** The drag started (past the threshold), or was abandoned. For the live region. */
  onStart?: (item: RunwayDragItem) => void;
  onAbandon?: () => void;
}

export interface RunwayDrag {
  dragging: RunwayDragItem | null;
  over: RunwayDropAt | null;
  /** Attach to a draggable card's `onPointerDown`. */
  press: (e: ReactPointerEvent, item: RunwayDragItem) => void;
}

/** How far the pointer moves before a press becomes a drag. */
export const DRAG_THRESHOLD_PX = 4;

/** Presses on these never start a drag: text fields, anything marked, and the menus and dialogs a
 *  card opens (portalled out of the card in the DOM, but React still bubbles their events to it). */
const NO_DRAG =
  "input, select, textarea, [data-runway-nodrag], [data-popover-panel], [role='dialog']";

/** The zone under (x, y) inside `root`, and the card the drop would go before. */
export function hitTest(
  root: HTMLElement,
  x: number,
  y: number,
  draggingKey: string | null,
): RunwayDropAt | null {
  const zones = root.querySelectorAll<HTMLElement>("[data-runway-zone]");
  for (const z of zones) {
    const r = z.getBoundingClientRect();
    if (x < r.left || x > r.right || y < r.top || y > r.bottom) continue;
    const zone = z.dataset.runwayZone ?? "";
    let target: RunwayDropAt["target"];
    if (z.dataset.runwayParked !== undefined) target = { kind: "parked" };
    else {
      const columnId = z.dataset.runwayCol;
      const lane = z.dataset.runwayLane;
      if (!columnId || (lane !== "foreground" && lane !== "background")) continue;
      target = { kind: "column", columnId, lane };
    }
    let index = 0;
    for (const c of z.querySelectorAll<HTMLElement>("[data-runway-card]")) {
      if (c.dataset.runwayCard !== zone) continue;
      if (draggingKey !== null && c.dataset.runwayKey === draggingKey) continue;
      const b = c.getBoundingClientRect();
      if (y > b.top + b.height / 2) index += 1;
    }
    return { zone, target, index };
  }
  return null;
}

export function useRunwayDrag(opts: RunwayDragOptions): RunwayDrag {
  const [dragging, setDragging] = useState<RunwayDragItem | null>(null);
  const [over, setOver] = useState<RunwayDropAt | null>(null);
  const optsRef = useRef(opts);
  optsRef.current = opts;
  // The live gesture's teardown, so an unmount mid-drag leaves no listener behind.
  const stopRef = useRef<(() => void) | null>(null);

  useEffect(() => () => stopRef.current?.(), []);

  const press = useCallback((e: ReactPointerEvent, item: RunwayDragItem) => {
    // The main button of the primary pointer (a second finger never starts a second drag).
    if (e.button !== 0 || e.isPrimary === false) return;
    if (e.target instanceof Element && e.target.closest(NO_DRAG)) return;
    stopRef.current?.();
    const startX = e.clientX;
    const startY = e.clientY;
    let active = false;

    const end = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onCancel);
      window.removeEventListener("keydown", onKey, true);
      window.removeEventListener("blur", onCancel);
      stopRef.current = null;
      setDragging(null);
      setOver(null);
    };
    const locate = (x: number, y: number) => {
      const root = optsRef.current.rootRef.current;
      return root ? hitTest(root, x, y, item.key) : null;
    };
    const onMove = (ev: PointerEvent) => {
      if (!active) {
        if (Math.hypot(ev.clientX - startX, ev.clientY - startY) < DRAG_THRESHOLD_PX) return;
        active = true;
        window.getSelection?.()?.removeAllRanges();
        setDragging(item);
        optsRef.current.onStart?.(item);
      }
      setOver(locate(ev.clientX, ev.clientY));
    };
    const onUp = (ev: PointerEvent) => {
      const was = active;
      const at = was ? locate(ev.clientX, ev.clientY) : null;
      end();
      if (!was) return;
      // The click that follows a drag's release is the drag's, not the card's.
      const swallow = (c: MouseEvent) => {
        c.stopPropagation();
        c.preventDefault();
      };
      window.addEventListener("click", swallow, { capture: true, once: true });
      setTimeout(() => window.removeEventListener("click", swallow, { capture: true }), 0);
      if (at) optsRef.current.onDrop(item, at);
      else optsRef.current.onAbandon?.();
    };
    const onCancel = () => {
      const was = active;
      end();
      if (was) optsRef.current.onAbandon?.();
    };
    const onKey = (ev: KeyboardEvent) => {
      if (ev.key !== "Escape" || !active) return;
      ev.preventDefault();
      ev.stopPropagation();
      onCancel();
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onCancel);
    window.addEventListener("keydown", onKey, true);
    window.addEventListener("blur", onCancel);
    stopRef.current = end;
  }, []);

  return { dragging, over, press };
}
