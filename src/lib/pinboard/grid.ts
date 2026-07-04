// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

/** The Pinboard's grid geometry and the pure snap/clamp/collision helpers. No React, no
 *  DOM — just math, so the drag/resize behaviour is easy to reason about and unit-test in
 *  isolation (in the spirit of the backend's pure cores). The board is **bounded**, but the
 *  extent is no longer a hard constant: `COLS`/`ROWS` are the *minimum* size (the legacy board),
 *  and the view grows the board to fill the window (see PinboardView). Every helper therefore
 *  takes the current `cols`/`rows` bounds, defaulting to the legacy floor for callers that don't
 *  care. The cell size (`CELL`) and every font stay fixed — only the number of cells changes. */

import type { Rect, Widget } from "./types";

/** Pixels per grid cell — fixed. The board's pixel size is cols×CELL by rows×CELL. */
export const CELL = 24;
/** The board's minimum extent in cells (the original fixed board); it never shrinks below this. */
export const COLS = 44;
export const ROWS = 28;

/** Smallest a widget may be shrunk to (cells), so a resize can't make it unusable. */
export const MIN_W = 4;
export const MIN_H = 3;

/** The board's cell extent for a pixel viewport, never smaller than the legacy COLS×ROWS floor.
 *  Pure — the caller supplies the measured/screen pixels (keeps this module DOM-free). */
export function boundsForPx(px: { w: number; h: number }): { cols: number; rows: number } {
  return {
    cols: Math.max(COLS, Math.floor(px.w / CELL)),
    rows: Math.max(ROWS, Math.floor(px.h / CELL)),
  };
}

/** Snap a pixel offset to the nearest whole cell. */
export function snap(px: number): number {
  return Math.round(px / CELL);
}

/** Clamp a rect to the board: enforce min size, then keep it fully inside cols×rows. */
export function clampRect(r: Rect, cols: number = COLS, rows: number = ROWS): Rect {
  const w = Math.max(MIN_W, Math.min(Math.round(r.w), cols));
  const h = Math.max(MIN_H, Math.min(Math.round(r.h), rows));
  const x = Math.max(0, Math.min(Math.round(r.x), cols - w));
  const y = Math.max(0, Math.min(Math.round(r.y), rows - h));
  return { x, y, w, h };
}

/** Convert a pixel rect (e.g. a live drag position) to a snapped, in-bounds cell rect. */
export function pxRectToCells(
  px: { x: number; y: number; w: number; h: number },
  cols: number = COLS,
  rows: number = ROWS,
): Rect {
  return clampRect({ x: snap(px.x), y: snap(px.y), w: snap(px.w), h: snap(px.h) }, cols, rows);
}

/** Do two cell rects overlap at all? (Half-open intervals — touching edges don't count.) */
export function rectsOverlap(a: Rect, b: Rect): boolean {
  return a.x < b.x + b.w && b.x < a.x + a.w && a.y < b.y + b.h && b.y < a.y + a.h;
}

/**
 * Find a free top-left slot for a new `w×h` widget by scanning the grid row-major, skipping
 * cells that would overlap an existing widget. Falls back to the origin (allowing overlap)
 * when the board is too full to fit it — the user can always drag it somewhere.
 */
export function findFreeRect(
  widgets: Widget[],
  w: number,
  h: number,
  cols: number = COLS,
  rows: number = ROWS,
): Rect {
  const cw = Math.min(w, cols);
  const ch = Math.min(h, rows);
  for (let y = 0; y <= rows - ch; y++) {
    for (let x = 0; x <= cols - cw; x++) {
      const candidate: Rect = { x, y, w: cw, h: ch };
      if (!widgets.some((widget) => rectsOverlap(candidate, widget.rect))) {
        return candidate;
      }
    }
  }
  return { x: 0, y: 0, w: cw, h: ch };
}
