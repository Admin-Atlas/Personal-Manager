// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

/** The Pinboard's grid geometry and the pure snap/clamp/collision helpers. No React, no
 *  DOM — just math, so the drag/resize behaviour is easy to reason about and unit-test in
 *  isolation (in the spirit of the backend's pure cores). The board is **bounded**, but the
 *  extent is no longer a hard constant: `COLS`/`ROWS` are the *minimum* size (the legacy board),
 *  and the view grows the board to fill the window (see PinboardView). Every helper therefore
 *  takes the current `cols`/`rows` bounds, defaulting to the legacy floor for callers that don't
 *  care. The cell size (`CELL`) and every font stay fixed — only the number of cells changes. */

import type { CellPoint, Rect, Widget, WidgetKind } from "./types";

/** Pixels per grid cell — fixed. The board's pixel size is cols×CELL by rows×CELL. */
export const CELL = 24;
/** The board's minimum extent in cells (the original fixed board); it never shrinks below this. */
export const COLS = 44;
export const ROWS = 28;

/** Smallest a note/timeline may be shrunk to (cells), so a resize can't make it unusable. */
export const MIN_W = 4;
export const MIN_H = 3;
/** A collapsed folder starts as a compact tile — smaller than a note. It IS resizable (every kind
 *  is); this is its default and its floor, not a fixed size. */
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

/** Is a cell point inside a rect? Half-open on the far edges, matching {@link rectsOverlap}, so a
 *  point on a rect's right/bottom boundary belongs to the neighbour, never to both. */
export function rectContains(r: Rect, p: CellPoint): boolean {
  return p.x >= r.x && p.x < r.x + r.w && p.y >= r.y && p.y < r.y + r.h;
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

/**
 * Keep the board a FIXED width of `cols` (the window): widgets already within the width stay exactly
 * where they are, and any that overhang the right edge re-flow into a free slot — scanned row-major,
 * so they wrap onto a new row rather than sitting off-screen. Only top-level x/w matter (folder
 * children aren't board-positioned). Used on load so a board authored on a wider window, or before
 * the fixed-width model, tidies itself into the current window.
 */
export function reflowToWidth(widgets: Widget[], cols: number, rows: number): Widget[] {
  const placed: Widget[] = [];
  for (const w of widgets) {
    if (w.rect.x + w.rect.w <= cols) {
      placed.push(w);
    } else {
      const rect = findFreeRect(placed, Math.min(w.rect.w, cols), w.rect.h, cols, rows);
      placed.push({ ...w, rect });
    }
  }
  return placed;
}

// --- folders (pure) ------------------------------------------------------------------------------

/**
 * The folder a move-drop would file into: the TOP-MOST folder whose rect contains the **pointer**.
 *
 * This is the single source of truth for "would this drop file into a folder?" — {@link resolveDrop}
 * and the view's during-drag highlight both call it, so what the user is shown and what actually
 * happens cannot drift apart.
 *
 * The pointer decides, not the dragged rect: a big note only *overlapping* a folder is placed
 * normally (widgets are free to overlap), so filing stays a deliberate aim rather than a graze.
 *
 * Returns undefined when there's no pointer (a drop with no known pointer must never file — see
 * usePinboard), nothing is under it, or the moving widget is itself a folder: **folders never nest,
 * so a dragged folder just stacks.**
 */
export function folderAtPointer(
  widgets: Widget[],
  movingId: string,
  pointer: CellPoint | null,
): Widget | undefined {
  if (!pointer) return undefined;
  const moving = widgets.find((w) => w.id === movingId);
  if (!moving || moving.kind === "folder") return undefined;
  // Array order IS paint order (raiseWidget appends), so search from the top down: with folders
  // free to stack, the one the user can actually see must win.
  return [...widgets]
    .reverse()
    .find((w) => w.id !== movingId && w.kind === "folder" && rectContains(w.rect, pointer));
}

/**
 * A card's landing spot INSIDE a folder: a free slot among its siblings, at the card's exact size.
 *
 * A child's rect is its position on the folder's own board (what the overlay lays out), so filing has
 * to choose one — the card's board rect is meaningless in there, and in the twin-fold case both cards
 * are at the *same* rect by definition, so keeping it would stack one exactly on top of the other.
 *
 * `cols`/`rows` are the outer board's extent, which is always ≥ any card that was sitting on it, so
 * findFreeRect's `Math.min` clamp can't fire and the size is preserved exactly — which is the whole
 * point of the folder board.
 */
function placeInFolder(siblings: Widget[], size: Rect, cols: number, rows: number): Rect {
  return findFreeRect(siblings, size.w, size.h, cols, rows);
}

/**
 * Resolve where a just-moved widget `id` lands at cell-`rect`, returning the next widget list:
 *   1. Pointer over a folder → file the widget into it (see {@link folderAtPointer}), laid out clear
 *      of its new siblings.
 *   2. Else another loose widget at the exact same rect → combine the two into a NEW folder at that
 *      spot (the deliberate "stack them to fold" gesture). Folders are excluded on BOTH sides: two
 *      folders share the default 3×3 tile, so they are exact-rect twins the moment they're stacked,
 *      and folding them would destroy both shells and merge their notes.
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
  pointer: CellPoint | null,
): Widget[] {
  const moving = widgets.find((w) => w.id === id);
  if (!moving) return widgets;
  const rest = widgets.filter((w) => w.id !== id);

  // 1) Pointer over a folder → file it in. (Never fires for a moving folder — no nesting.)
  const folder = folderAtPointer(widgets, id, pointer);
  if (folder) {
    const kids = folder.children ?? [];
    const filed: Widget = { ...moving, rect: placeInFolder(kids, moving.rect, cols, rows) };
    return rest.map((w) => (w.id === folder.id ? { ...w, children: [...kids, filed] } : w));
  }

  // 2) Exact-rect twin → fold the two identically-placed widgets into a new folder.
  const twin =
    moving.kind === "folder"
      ? undefined
      : rest.find(
          (w) =>
            w.kind !== "folder" &&
            w.rect.x === rect.x &&
            w.rect.y === rect.y &&
            w.rect.w === rect.w &&
            w.rect.h === rect.h,
        );
  if (twin) {
    const folderRect = clampRect(
      { x: rect.x, y: rect.y, w: FOLDER_W, h: FOLDER_H },
      cols,
      rows,
      FOLDER_MIN,
    );
    // The two are at the SAME rect — that's the gesture — so they must be laid out afresh inside the
    // folder, or the overlay would render one exactly on top of the other.
    const first: Widget = { ...twin, rect: placeInFolder([], twin.rect, cols, rows) };
    const second: Widget = { ...moving, rect: placeInFolder([first], moving.rect, cols, rows) };
    const newFolder: Widget = {
      id: makeId(),
      kind: "folder",
      rect: folderRect,
      title: "",
      children: [first, second],
      expandMode: "inline",
    };
    return [...rest.filter((w) => w.id !== twin.id), newFolder];
  }

  // 3) Plain move (keep array position; raiseWidget already ran on grab). Two folders now stack
  //    rather than merge — but a folder landing EXACTLY on another is pixel-identical and would hide
  //    it completely, with no way to grab the one underneath, so nudge it clear by a cell.
  const landing =
    moving.kind === "folder" &&
    rest.some((w) => w.kind === "folder" && w.rect.x === rect.x && w.rect.y === rect.y)
      ? clampRect({ ...rect, x: rect.x + 1, y: rect.y + 1 }, cols, rows, minSize(moving.kind))
      : rect;
  // Unchanged landing → hand back the same array, so a click-without-moving stays a no-op.
  if (
    landing.x === moving.rect.x &&
    landing.y === moving.rect.y &&
    landing.w === moving.rect.w &&
    landing.h === moving.rect.h
  ) {
    return widgets;
  }
  return widgets.map((w) => (w.id === id ? { ...w, rect: landing } : w));
}
