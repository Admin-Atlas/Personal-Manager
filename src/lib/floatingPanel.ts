// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Geometry for a free-floating, draggable, resizable in-app panel.
//
// Kept pure and separate from the component (like `pinboard/grid.ts`) so the rules that are easy to
// get wrong — and impossible to notice until a user has the wrong monitor — are unit-tested:
//
//  * the panel must never cover the title bar, because that strip carries the frameless window's
//    `data-tauri-drag-region` and its minimise / maximise / close buttons. Lose it and the user
//    cannot move or close the app window.
//  * geometry saved on a large monitor must not strand the panel off-screen on a smaller one. It is
//    re-clamped on every window resize and on load, never trusted as stored.

export interface PanelRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface Viewport {
  w: number;
  h: number;
}

/** The custom title bar is `h-9` (36px); app-scope overlays start below it by convention. */
export const TITLE_BAR_H = 36;

export const MIN_W = 240;
export const MIN_H = 140;

/** Keep at least this much of the panel reachable when the viewport is smaller than the minimum. */
const EDGE_KEEP = 24;

/**
 * Clamp a panel rect into the usable area: at or below the title bar, and fully on screen.
 *
 * Size is clamped first, then position — otherwise a panel wider than the viewport would be pushed
 * to x=0 and then still overflow to the right. On a viewport too small for the minimum size the
 * panel is allowed to overflow the bottom/right rather than shrinking below usability, but its
 * top-left stays reachable so it can always be dragged back.
 */
export function clampPanel(rect: PanelRect, view: Viewport): PanelRect {
  const maxW = Math.max(MIN_W, view.w - EDGE_KEEP);
  const maxH = Math.max(MIN_H, view.h - TITLE_BAR_H - EDGE_KEEP);
  const w = Math.min(Math.max(rect.w, MIN_W), maxW);
  const h = Math.min(Math.max(rect.h, MIN_H), maxH);

  const maxX = Math.max(0, view.w - w);
  const maxY = Math.max(TITLE_BAR_H, view.h - h);
  const x = Math.min(Math.max(rect.x, 0), maxX);
  const y = Math.min(Math.max(rect.y, TITLE_BAR_H), maxY);

  return { x, y, w, h };
}

/** Where a panel first appears: near the top-right, clear of the title bar and the window edge. */
export function defaultPanelRect(view: Viewport): PanelRect {
  const w = 320;
  const h = 260;
  return clampPanel({ x: view.w - w - 24, y: TITLE_BAR_H + 16, w, h }, view);
}

/** Apply a drag delta to a rect (move: position only). */
export function movePanel(start: PanelRect, dx: number, dy: number, view: Viewport): PanelRect {
  return clampPanel({ ...start, x: start.x + dx, y: start.y + dy }, view);
}

/** Apply a resize delta to a rect, dragging the bottom-right corner (position fixed). */
export function resizePanel(start: PanelRect, dx: number, dy: number, view: Viewport): PanelRect {
  return clampPanel({ ...start, w: start.w + dx, h: start.h + dy }, view);
}

const KEY = "pm.briefing.panelRect";

/** The stored rect, re-clamped to the CURRENT viewport, or the default when absent/unusable. */
export function readPanelRect(view: Viewport): PanelRect {
  try {
    const raw = localStorage.getItem(KEY);
    if (raw) {
      const p: unknown = JSON.parse(raw);
      if (p && typeof p === "object") {
        const r = p as Record<string, unknown>;
        const nums = [r.x, r.y, r.w, r.h];
        if (nums.every((n) => typeof n === "number" && Number.isFinite(n))) {
          return clampPanel(
            { x: r.x as number, y: r.y as number, w: r.w as number, h: r.h as number },
            view,
          );
        }
      }
    }
  } catch {
    /* fall through to the default */
  }
  return defaultPanelRect(view);
}

export function writePanelRect(rect: PanelRect): void {
  try {
    localStorage.setItem(KEY, JSON.stringify(rect));
  } catch {
    /* best-effort — it just won't be remembered */
  }
}
