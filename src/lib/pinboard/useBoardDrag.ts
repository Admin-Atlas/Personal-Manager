// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The pointer drag/resize gesture for a pinboard surface, hand-rolled on pointer events (no layout
// library) — extracted from PinboardView so the main board and the folder overlay's mini-board can
// share ONE implementation rather than growing two that drift apart. The snap/clamp maths stays in
// grid.ts; this owns only the gesture: what's being dragged, where it is right now, and what the
// board should be told on release.
//
// Two things every caller gets for free:
//   - the gesture survives the pointer leaving the widget (listeners are on `window`), and is
//     committed or cleanly abandoned on pointercancel/blur — see the handlers below;
//   - the pointer's board CELL is reported on release, which is what decides whether a drop files
//     into a folder (grid.folderAtPointer). The surface's origin is read LIVE per event rather than
//     cached at grab: both boards live in scrollers the user can wheel mid-drag, and a stale origin
//     would let the drop disagree with the highlight the user was shown.

import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { CELL, minSize, pxRectToCells } from "./grid";
import type { CellPoint, Rect, Widget, WidgetKind } from "./types";

export interface PxRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export type DragMode = "move" | "resize";

interface DragStart {
  id: string;
  /** The dragged widget's kind, captured at grab so the drop can pick the right min size (folders
   *  floor at 3×3) without the pointer handlers needing to look the widget up. */
  kind: WidgetKind;
  mode: DragMode;
  startX: number;
  startY: number;
  startRect: Rect;
}

export interface BoardDragOptions {
  /** The surface's cell extent, read through a ref so a live gesture never re-subscribes. */
  boundsRef: { current: { cols: number; rows: number } };
  /** The board canvas element — its viewport origin turns a pointer into a board cell. */
  surfaceRef: { current: HTMLElement | null };
  /** A move gesture ended. `pointer` is the board cell under the cursor, or null if unknowable. */
  onMoveEnd: (id: string, rect: Rect, pointer: CellPoint | null) => void;
  /** A resize gesture ended. */
  onResizeEnd: (id: string, rect: Rect) => void;
  /** Fired on grab, before the gesture — used to raise the widget to the top of the paint order. */
  onGrab?: (id: string) => void;
  /** Live pointer cell during a MOVE, for a drop-target highlight. Null when there's nothing to aim
   *  at (or during a resize). Omit if the surface has no folders to file into. */
  onPointerCell?: (id: string, pointer: CellPoint | null) => void;
}

export interface BoardDrag {
  /** The in-flight gesture's widget id, or null. */
  draggingId: string | null;
  /** The dragged widget's live pixel rect, or null between gestures. */
  livePx: PxRect | null;
  /** Attach to a widget's header (move) and resize handle (resize). */
  startDrag: (e: ReactPointerEvent, w: Widget, mode: DragMode) => void;
}

export function rectToPx(r: Rect): PxRect {
  return { x: r.x * CELL, y: r.y * CELL, w: r.w * CELL, h: r.h * CELL };
}

export function useBoardDrag(opts: BoardDragOptions): BoardDrag {
  const [drag, setDrag] = useState<DragStart | null>(null);
  const [livePx, setLivePx] = useState<PxRect | null>(null);

  // The callbacks go through a ref so the pointer effect depends only on the gesture itself. A
  // caller passing an inline arrow would otherwise re-subscribe the listeners on every render — and
  // this hook's whole job is to keep one gesture stable from pointerdown to pointerup.
  const optsRef = useRef(opts);
  optsRef.current = opts;

  useEffect(() => {
    if (!drag) return;
    const startPx = rectToPx(drag.startRect);
    const compute = (e: PointerEvent): PxRect => {
      const { cols, rows } = optsRef.current.boundsRef.current;
      const dx = e.clientX - drag.startX;
      const dy = e.clientY - drag.startY;
      const maxX = cols * CELL;
      const maxY = rows * CELL;
      if (drag.mode === "move") {
        return {
          x: Math.max(0, Math.min(startPx.x + dx, maxX - startPx.w)),
          y: Math.max(0, Math.min(startPx.y + dy, maxY - startPx.h)),
          w: startPx.w,
          h: startPx.h,
        };
      }
      // Clamp to the kind's own minimum so a folder can shrink back to its 3×3 floor (not the
      // note/timeline 4×3) while still growing freely.
      const min = minSize(drag.kind);
      return {
        x: startPx.x,
        y: startPx.y,
        w: Math.max(min.w * CELL, Math.min(startPx.w + dx, maxX - startPx.x)),
        h: Math.max(min.h * CELL, Math.min(startPx.h + dy, maxY - startPx.y)),
      };
    };
    // The board cell under the pointer. Only meaningful for a move — a resize files nothing.
    const pointerCell = (e: PointerEvent): CellPoint | null => {
      const el = optsRef.current.surfaceRef.current;
      if (!el || drag.mode !== "move") return null;
      const r = el.getBoundingClientRect();
      return {
        x: Math.floor((e.clientX - r.left) / CELL),
        y: Math.floor((e.clientY - r.top) / CELL),
      };
    };

    const onMove = (e: PointerEvent) => {
      setLivePx(compute(e));
      optsRef.current.onPointerCell?.(drag.id, pointerCell(e));
    };
    const onUp = (e: PointerEvent) => {
      const { cols, rows } = optsRef.current.boundsRef.current;
      const rect = pxRectToCells(compute(e), cols, rows, minSize(drag.kind));
      if (drag.mode === "resize") optsRef.current.onResizeEnd(drag.id, rect);
      else optsRef.current.onMoveEnd(drag.id, rect, pointerCell(e));
      setDrag(null);
      setLivePx(null);
      optsRef.current.onPointerCell?.(drag.id, null);
    };
    // If the gesture is interrupted (touch handed to the scroller, an OS context menu, the window
    // losing focus), the browser fires pointercancel/blur instead of pointerup. Without this the
    // drag would dangle: `drag` stuck non-null, `select-none` stuck on, and the move silently lost.
    // pointercancel still carries coordinates so we commit; a blur has none, so we just end cleanly.
    const onCancel = (e: PointerEvent) => onUp(e);
    const onBlur = () => {
      setDrag(null);
      setLivePx(null);
      optsRef.current.onPointerCell?.(drag.id, null);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onCancel);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onCancel);
      window.removeEventListener("blur", onBlur);
    };
  }, [drag]);

  // Stable across the per-tick drag re-renders, so passing it into each memoised widget body doesn't
  // defeat the memo that keeps react-markdown from re-running on every pointermove.
  const startDrag = useCallback((e: ReactPointerEvent, w: Widget, mode: DragMode) => {
    e.preventDefault();
    optsRef.current.onGrab?.(w.id);
    setDrag({
      id: w.id,
      kind: w.kind,
      mode,
      startX: e.clientX,
      startY: e.clientY,
      startRect: w.rect,
    });
    setLivePx(rectToPx(w.rect));
  }, []);

  return { draggingId: drag?.id ?? null, livePx, startDrag };
}
