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
}

/**
 * A panel width stored as a *fraction of the window*, never a pixel count, so it stays
 * proportional as the window resizes (per the "relative, not pixel-locked" requirement).
 * Drag-resizable from one edge via a window-level pointer capture (mirrors the Pinboard
 * drag), clamped to `[minFrac, maxFrac]`, and remembered on this device.
 *
 * Returns the live pixel `width` to apply (`style={{ width }}`), a `startResize`
 * pointer-down handler for the grab handle, and `resizing` for cursor/select-none feedback.
 */
export function useResizable({ storageKey, defaultFrac, minFrac, maxFrac, edge }: Options) {
  const [frac, setFrac] = useState(() => {
    const raw = Number(localStorage.getItem(storageKey));
    return clamp(Number.isFinite(raw) && raw > 0 ? raw : defaultFrac, minFrac, maxFrac);
  });
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

  const startResize = useCallback(
    (e: React.PointerEvent) => {
      e.preventDefault();
      const startX = e.clientX;
      const startWidth = clamp(fracRef.current, minFrac, maxFrac) * window.innerWidth;
      setResizing(true);
      const onMove = (ev: PointerEvent) => {
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
      };
      window.addEventListener("pointermove", onMove);
      window.addEventListener("pointerup", finish);
      window.addEventListener("pointercancel", finish);
      window.addEventListener("blur", finish);
    },
    [edge, minFrac, maxFrac, storageKey],
  );

  return { width: clamp(frac, minFrac, maxFrac) * vw, startResize, resizing };
}
