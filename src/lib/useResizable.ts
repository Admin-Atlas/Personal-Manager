// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useRef, useState } from "react";

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

interface Options {
  /** localStorage key the chosen fraction is remembered under (on this device). */
  storageKey: string;
  /** Starting width as a fraction of the window (0–1). */
  defaultFrac: number;
  /** Narrowest / widest the panel may be, as fractions of the window. */
  minFrac: number;
  maxFrac: number;
  /**
   * Which edge carries the drag handle. A right-docked panel uses `"left"`: dragging the
   * handle leftwards (negative dx) widens the panel; a left-docked panel uses `"right"`.
   */
  edge: "left" | "right";
  /**
   * When set, the panel can be *snap-collapsed*: once it's already at its minimum width and the
   * user keeps dragging the handle past the outer {@link collapseThreshold} of the window (toward
   * the edge the panel is docked to), it snaps shut. The caller then shows a small reopen affordance
   * and calls {@link ResizableResult.expand} to bring it back at the minimum width.
   */
  collapsible?: boolean;
  /** Fraction of the window width near the docked edge that triggers a snap-collapse (default 0.05). */
  collapseThreshold?: number;
}

interface ResizableResult {
  /** Live pixel width to apply to the panel (`style={{ width }}`). Meaningless while collapsed. */
  width: number;
  /** Pointer-down handler for the grab handle. */
  startResize: (e: React.PointerEvent) => void;
  /** True mid-drag, for cursor / select-none feedback. */
  resizing: boolean;
  /** True while the panel is snap-collapsed (only ever true when `collapsible`). */
  collapsed: boolean;
  /** Reopen a collapsed panel at its minimum width (so the user can drag it wider again). */
  expand: () => void;
}

/**
 * A panel width stored as a *fraction of the window*, never a pixel count, so it stays
 * proportional as the window resizes (per the "relative, not pixel-locked" requirement).
 * Drag-resizable from one edge via a window-level pointer capture (mirrors the Pinboard
 * drag), clamped to `[minFrac, maxFrac]`, and remembered on this device. Optionally
 * snap-collapsible by dragging past the window edge (see {@link Options.collapsible}).
 */
export function useResizable({
  storageKey,
  defaultFrac,
  minFrac,
  maxFrac,
  edge,
  collapsible = false,
  collapseThreshold = 0.05,
}: Options): ResizableResult {
  const collapsedKey = `${storageKey}.collapsed`;
  const [frac, setFrac] = useState(() => {
    const raw = Number(localStorage.getItem(storageKey));
    return clamp(Number.isFinite(raw) && raw > 0 ? raw : defaultFrac, minFrac, maxFrac);
  });
  const [collapsed, setCollapsed] = useState(
    () => collapsible && localStorage.getItem(collapsedKey) === "true",
  );
  const [resizing, setResizing] = useState(false);
  const [vw, setVw] = useState(() => window.innerWidth);
  const fracRef = useRef(frac);
  fracRef.current = frac;

  // Keep the pixel width proportional when the window itself resizes (also re-clamps, so a
  // wide saved fraction can't strand the panel off-screen on a smaller window).
  useEffect(() => {
    const onResize = () => setVw(window.innerWidth);
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  const expand = useCallback(() => {
    setCollapsed(false);
    setFrac(minFrac); // reopen at the minimum so the user can drag it back out
    localStorage.setItem(collapsedKey, "false");
    localStorage.setItem(storageKey, String(minFrac));
  }, [collapsedKey, minFrac, storageKey]);

  const startResize = useCallback(
    (e: React.PointerEvent) => {
      e.preventDefault();
      const startX = e.clientX;
      const startWidth = clamp(fracRef.current, minFrac, maxFrac) * window.innerWidth;
      setResizing(true);
      let collapsedDuringDrag = false;
      const onMove = (ev: PointerEvent) => {
        if (collapsedDuringDrag) return; // already snapped shut this gesture — ignore the rest of it
        // Snap shut once the pointer reaches the outer sliver of the window nearest the docked edge
        // (the panel is already pinned at min width by then, so this only fires on an over-drag).
        if (collapsible) {
          const w = window.innerWidth;
          const inCollapseZone =
            edge === "left"
              ? ev.clientX >= w * (1 - collapseThreshold) // right-docked: drag toward the right edge
              : ev.clientX <= w * collapseThreshold; // left-docked: drag toward the left edge
          if (inCollapseZone) {
            collapsedDuringDrag = true;
            setCollapsed(true);
            return; // the natural pointerup ends the gesture and persists the collapsed state
          }
        }
        const dx = ev.clientX - startX;
        const nextPx = edge === "left" ? startWidth - dx : startWidth + dx;
        setFrac(clamp(nextPx / window.innerWidth, minFrac, maxFrac));
      };
      // Commit once on release; pointercancel/blur end a gesture handed off to the OS.
      const finish = () => {
        window.removeEventListener("pointermove", onMove);
        window.removeEventListener("pointerup", finish);
        window.removeEventListener("pointercancel", finish);
        window.removeEventListener("blur", finish);
        setResizing(false);
        localStorage.setItem(storageKey, String(fracRef.current));
        if (collapsible) localStorage.setItem(collapsedKey, String(collapsedDuringDrag));
      };
      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", finish);
      window.addEventListener("pointercancel", finish);
      window.addEventListener("blur", finish);
    },
    [edge, minFrac, maxFrac, storageKey, collapsible, collapseThreshold, collapsedKey],
  );

  return { width: clamp(frac, minFrac, maxFrac) * vw, startResize, resizing, collapsed, expand };
}
