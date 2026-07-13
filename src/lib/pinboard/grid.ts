// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

/** The Pinboard's grid geometry and the pure snap/clamp/collision helpers. No React, no
 *  DOM — just math, so the drag/resize behaviour is easy to reason about and unit-test in
 *  isolation (in the spirit of the backend's pure cores). The board is **bounded**, but the
 *  extent is no longer a hard constant: `COLS`/`ROWS` are the *minimum* size (the legacy board),
 *  and the view grows the board to fill the window (see PinboardView). Every helper therefore
 *  takes the current `cols`/`rows` bounds, defaulting to the legacy floor for callers that don't
 *  care. The cell size (`CELL`) and every font stay fixed — only the number of cells changes. */

import type { Rect, Widget, WidgetKind } from "./types";

/** Pixels per grid cell — fixed. The board's pixel size is cols×CELL by rows×CELL. */
export const CELL = 24;
/** The board's minimum extent in cells (the original fixed board); it never shrinks below this. */
export const COLS = 44;
export const ROWS = 28;

/** Smallest a note/timeline may be shrunk to (cells), so a resize can't make it unusable. */
export const MIN_W = 4;
export const MIN_H = 3;
/** A collapsed folder is a fixed, compact tile — smaller than a note (it isn't resizable). */
export const FOLDER_W = 3;
export const FOLDER_H = 3;

/** A widget's minimum cell size. */
export interface Min {
  w: number;
  h: number;
}
export const DEFAULT_MIN: Min = { w: MIN_W, h: MIN_H };
export const FOLDER_MIN: Min = { w: FOLDER_W, h: FOLDER_H };

/** The minimum cell size for a widget kind — folders floor at 3×3, everything else at MIN_W×MIN_H.
 *  Threaded through the clamp path so a 3×3 folder tile survives load instead of being bumped to 4. */
export function minSize(kind: WidgetKind): Min {
  return kind === "folder" ? FOLDER_MIN : DEFAULT_MIN;
}

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

/** Clamp a rect to the board: enforce min size, then keep it fully inside cols×rows. `min` defaults
 *  to the note/timeline floor; folders pass {@link FOLDER_MIN} so their 3×3 tile isn't bumped up. */
export function clampRect(
  r: Rect,
  cols: number = COLS,
  rows: number = ROWS,
  min: Min = DEFAULT_MIN,
): Rect {
  const w = Math.max(min.w, Math.min(Math.round(r.w), cols));
  const h = Math.max(min.h, Math.min(Math.round(r.h), rows));
  const x = Math.max(0, Math.min(Math.round(r.x), cols - w));
  const y = Math.max(0, Math.min(Math.round(r.y), rows - h));
  return { x, y, w, h };
}

/** Convert a pixel rect (e.g. a live drag position) to a snapped, in-bounds cell rect. */
export function pxRectToCells(
  px: { x: number; y: number; w: number; h: number },
  cols: number = COLS,
  rows: number = ROWS,
  min: Min = DEFAULT_MIN,
): Rect {
  return clampRect({ x: snap(px.x), y: snap(px.y), w: snap(px.w), h: snap(px.h) }, cols, rows, min);
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

// --- folders (pure) ------------------------------------------------------------------------------

/** The widgets a container contributes to a merge: a folder yields its children (never itself, so
 *  folders can't nest), anything else yields itself. */
function flatten(w: Widget): Widget[] {
  return w.kind === "folder" ? (w.children ?? []) : [w];
}

/**
 * Normalise a widget list so no folder is left with ≤1 child (auto-dissolve). A folder reduced to a
 * single child is replaced by that child at the folder's own position; an empty folder disappears.
 * Run after any child removal / pop-out and on load, so a hand-edited or drained folder self-heals.
 */
export function dissolveFolders(
  widgets: Widget[],
  cols: number = COLS,
  rows: number = ROWS,
): Widget[] {
  return widgets.flatMap((w) => {
    if (w.kind !== "folder") return [w];
    const kids = w.children ?? [];
    if (kids.length >= 2) return [w];
    if (kids.length === 1) {
      const c = kids[0];
      return [
        {
          ...c,
          rect: clampRect({ ...c.rect, x: w.rect.x, y: w.rect.y }, cols, rows, minSize(c.kind)),
        },
      ];
    }
    return []; // 0 children → the folder is gone
  });
}

/**
 * Resolve where a just-moved widget `id` lands at cell-`rect`, returning the next widget list:
 *   1. If the drop overlaps an existing folder → add the widget into that folder (folder-onto-folder
 *      flattens the moved folder's children in; no nesting).
 *   2. Else if another loose widget sits at the exact same rect → combine both into a NEW folder at
 *      that spot (the deliberate "stack them to fold" gesture).
 *   3. Else a plain move.
 * Pure: `makeId` is supplied so the function stays deterministic and unit-testable.
 */
export function resolveDrop(
  widgets: Widget[],
  id: string,
  rect: Rect,
  cols: number,
  rows: number,
  makeId: () => string,
): Widget[] {
  const moving = widgets.find((w) => w.id === id);
  if (!moving) return widgets;
  const rest = widgets.filter((w) => w.id !== id);

  // 1) Drop onto a folder → add (also handles folder-onto-folder by flattening; the moved shell goes).
  const folder = rest.find((w) => w.kind === "folder" && rectsOverlap(rect, w.rect));
  if (folder) {
    return rest.map((w) =>
      w.id === folder.id ? { ...w, children: [...(w.children ?? []), ...flatten(moving)] } : w,
    );
  }

  // 2) Exact-rect twin → make a folder from the two identically-placed widgets. (A folder twin is
  //    always caught by (1) first, since an exact match is an overlap, so `twin` is never a folder.)
  const twin = rest.find(
    (w) => w.rect.x === rect.x && w.rect.y === rect.y && w.rect.w === rect.w && w.rect.h === rect.h,
  );
  if (twin) {
    const children = [...flatten(twin), ...flatten(moving)];
    const folderRect = clampRect(
      { x: rect.x, y: rect.y, w: FOLDER_W, h: FOLDER_H },
      cols,
      rows,
      FOLDER_MIN,
    );
    const newFolder: Widget = {
      id: makeId(),
      kind: "folder",
      rect: folderRect,
      title: "",
      children,
      expandMode: "inline",
    };
    return [...rest.filter((w) => w.id !== twin.id), newFolder];
  }

  // 3) Plain move (keep array position; raiseWidget already ran on grab).
  return widgets.map((w) => (w.id === id ? { ...w, rect } : w));
}
