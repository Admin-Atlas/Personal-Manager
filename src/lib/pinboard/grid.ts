// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

/** The Pinboard's grid geometry and the pure snap/clamp/collision helpers. No React, no
 *  DOM — just math, so the drag/resize behaviour is easy to reason about and unit-test in
 *  isolation (in the spirit of the backend's pure cores). The board is **bounded**: a fixed
 *  COLS×ROWS grid, so widgets can never drift off into an infinite canvas — they clamp to
 *  the edges. */

import type { Rect, Widget } from "./types";

/** Pixels per grid cell. The board's pixel size is COLS×CELL by ROWS×CELL. */
export const CELL = 24;
export const COLS = 44;
export const ROWS = 28;

/** Smallest a widget may be shrunk to (cells), so a resize can't make it unusable. */
export const MIN_W = 4;
export const MIN_H = 3;

/** Snap a pixel offset to the nearest whole cell. */
export function snap(px: number): number {
  return Math.round(px / CELL);
}

/** Clamp a rect to the board: enforce min size, then keep it fully inside COLS×ROWS. */
export function clampRect(r: Rect): Rect {
  const w = Math.max(MIN_W, Math.min(Math.round(r.w), COLS));
  const h = Math.max(MIN_H, Math.min(Math.round(r.h), ROWS));
  const x = Math.max(0, Math.min(Math.round(r.x), COLS - w));
  const y = Math.max(0, Math.min(Math.round(r.y), ROWS - h));
  return { x, y, w, h };
}

/** Convert a pixel rect (e.g. a live drag position) to a snapped, in-bounds cell rect. */
export function pxRectToCells(px: { x: number; y: number; w: number; h: number }): Rect {
  return clampRect({ x: snap(px.x), y: snap(px.y), w: snap(px.w), h: snap(px.h) });
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
export function findFreeRect(widgets: Widget[], w: number, h: number): Rect {
  const cw = Math.min(w, COLS);
  const ch = Math.min(h, ROWS);
  for (let y = 0; y <= ROWS - ch; y++) {
    for (let x = 0; x <= COLS - cw; x++) {
      const candidate: Rect = { x, y, w: cw, h: ch };
      if (!widgets.some((widget) => rectsOverlap(candidate, widget.rect))) {
        return candidate;
      }
    }
  }
  return { x: 0, y: 0, w: cw, h: ch };
}
