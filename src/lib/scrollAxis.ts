// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A global, app-wide wheel normaliser so vertical and horizontal scrolling always do the intuitive
// thing — installed ONCE at app scope (see App.tsx) so every current and future table, list, or
// scroll region gets it for free, with no per-surface wiring.
//
// The rule this enforces:
//   • Find the nearest scroller under the pointer (either axis).
//   • A vertical wheel drives it up/down when it scrolls vertically. When that nearest scroller can
//     ONLY scroll horizontally (a wide table / row with no vertical scroll of its own), the vertical
//     wheel PANS it sideways instead — so a plain mouse with no tilt/side wheel can still reach the
//     far columns. At the horizontal edge we hand back to a vertical scroller further up so the page
//     keeps moving past the table.
//   • A horizontal wheel (a tilt wheel / MX-Master side-scroll, |deltaX| ≥ |deltaY|) drives the
//     nearest horizontal scroller left/right.
//   • Ctrl+wheel (zoom) and Shift+wheel (the native "wheel scrolls horizontally" convention) are left
//     entirely alone — GraphView / the semantic Map bind these for zoom & pan themselves.
//
// Why it's authoritative (finds the scroller and moves it itself, rather than trusting the browser):
// the browser's own axis mapping is exactly what's wrong here, so we can't rely on it. We only ever
// act when we FIND a scroller under the pointer; if there's none (e.g. the pointer is over the
// Map/Graph canvas, which owns its wheel for zoom/pan), we do nothing and let that element's own
// handler run — we never preventDefault or stopPropagation in that case.

const SCROLLABLE_OVERFLOW = /(auto|scroll|overlay)/;

/** Whether `el` can actually scroll on the given axis right now (scrollable overflow + real overflow). */
function canScroll(el: HTMLElement, axis: "x" | "y"): boolean {
  const style = getComputedStyle(el);
  if (axis === "y") {
    return SCROLLABLE_OVERFLOW.test(style.overflowY) && el.scrollHeight - el.clientHeight > 1;
  }
  return SCROLLABLE_OVERFLOW.test(style.overflowX) && el.scrollWidth - el.clientWidth > 1;
}

/**
 * The nearest ancestor (inclusive of `start`) that can currently scroll on `axis`, or null. Note we
 * deliberately DON'T fall back to the document root: this app keeps its shell fixed and scrolls inside
 * each view's own overflow container, and skipping the root fallback keeps us clear of the Map/Graph
 * canvas (returning null there lets its own wheel handler zoom/pan instead of us hijacking it).
 */
function nearestScroller(start: HTMLElement | null, axis: "x" | "y"): HTMLElement | null {
  let el = start;
  while (el && el !== document.body && el !== document.documentElement) {
    if (canScroll(el, axis)) return el;
    el = el.parentElement;
  }
  return null;
}

/**
 * The nearest ancestor (inclusive of `start`) that can scroll on EITHER axis, with a flag for each.
 * Same walk and root exclusion as {@link nearestScroller}; used so a vertical wheel over a
 * horizontal-only scroller (a wide table) can be redirected into sideways panning.
 */
function nearestAnyScroller(
  start: HTMLElement | null,
): { el: HTMLElement; x: boolean; y: boolean } | null {
  let el = start;
  while (el && el !== document.body && el !== document.documentElement) {
    const x = canScroll(el, "x");
    const y = canScroll(el, "y");
    if (x || y) return { el, x, y };
    el = el.parentElement;
  }
  return null;
}

/** Convert a wheel delta to pixels regardless of the reported deltaMode (pixel / line / page). */
function deltaToPixels(delta: number, mode: number, viewportExtent: number): number {
  if (mode === 1) return delta * 16; // lines → ~16px each
  if (mode === 2) return delta * viewportExtent; // pages
  return delta; // already pixels
}

function onWheel(e: WheelEvent): void {
  // Zoom (Ctrl) and the native shift-to-scroll-horizontally convention are left to the browser /
  // the canvas's own handler.
  if (e.ctrlKey || e.shiftKey) return;
  const absX = Math.abs(e.deltaX);
  const absY = Math.abs(e.deltaY);
  if (absX === 0 && absY === 0) return;

  const target = e.target instanceof HTMLElement ? e.target : null;
  if (!target) return;

  // The nearest scroller under the pointer, of either axis. If there's none — the Map/Graph canvas,
  // which owns its wheel for zoom/pan — do nothing so that element's own handler runs.
  const near = nearestAnyScroller(target);
  if (!near) return;

  const wantAxis: "x" | "y" = absY > absX ? "y" : "x";

  if (wantAxis === "y") {
    // Nearest scroller can move vertically → drive it vertically.
    if (near.y) {
      near.el.scrollTop += deltaToPixels(e.deltaY, e.deltaMode, near.el.clientHeight);
      e.preventDefault();
      return;
    }
    // Nearest scroller is horizontal-only → translate the vertical wheel into sideways panning so a
    // plain mouse (no tilt/side wheel) can reach the far columns. At the horizontal edge, hand back to
    // a vertical scroller further up so the page keeps scrolling past a wide table.
    const el = near.el;
    const atStart = el.scrollLeft <= 0;
    const atEnd = el.scrollLeft + el.clientWidth >= el.scrollWidth - 1;
    if ((e.deltaY < 0 && atStart) || (e.deltaY > 0 && atEnd)) {
      const up = nearestScroller(el.parentElement, "y");
      if (up) {
        up.scrollTop += deltaToPixels(e.deltaY, e.deltaMode, up.clientHeight);
        e.preventDefault();
      }
      return; // no vertical scroller above → let the browser's default run
    }
    el.scrollLeft += deltaToPixels(e.deltaY, e.deltaMode, el.clientWidth);
    e.preventDefault();
    return;
  }

  // Horizontal wheel → drive the nearest horizontal scroller.
  if (near.x) {
    near.el.scrollLeft += deltaToPixels(e.deltaX, e.deltaMode, near.el.clientWidth);
    e.preventDefault();
    return;
  }
  // Nearest scroller is vertical-only → look further up for a horizontal one; otherwise leave it.
  const up = nearestScroller(near.el.parentElement, "x");
  if (up) {
    up.scrollLeft += deltaToPixels(e.deltaX, e.deltaMode, up.clientWidth);
    e.preventDefault();
  }
}

/**
 * Install the global wheel-axis normaliser. Returns a disposer. Non-passive + capture so it can both
 * see the event first and cancel the browser's default (mis-)mapping before it happens.
 */
export function installAxisScrollNormalizer(): () => void {
  window.addEventListener("wheel", onWheel, { passive: false, capture: true });
  return () => window.removeEventListener("wheel", onWheel, { capture: true });
}
