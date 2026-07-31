// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A global, app-wide wheel normaliser so vertical and horizontal scrolling always do the intuitive
// thing — installed ONCE at app scope (see App.tsx) so every current and future table, list, or
// scroll region gets it for free, with no per-surface wiring.
//
// The rule this enforces:
//   • Find the nearest scroller under the pointer (either axis).
//   • A vertical wheel drives it up/down when it scrolls vertically — until that scroller is
//     EXHAUSTED in the wheel's own direction, where we hand the event back untouched so the
//     browser's native scroll chaining moves an ancestor (and honours any overscroll-behavior).
//     Cancelling there instead is what stopped the page dead under an exhausted nested list.
//   • When that nearest scroller can ONLY scroll horizontally (a wide table / row with no vertical
//     scroll of its own), the vertical wheel PANS it sideways instead — so a plain mouse with no
//     tilt/side wheel can still reach the far columns. At the horizontal edge we hand back to a
//     vertical scroller further up, MANUALLY, so the page keeps moving past the table.
//   • The asymmetry between those two is deliberate: we hand back to the browser only where the
//     browser's own axis mapping is already right (a vertical wheel over a vertical scroller). The
//     sideways-panning case is precisely the mapping it gets wrong — WebKit flips it, Blink does
//     not — so there we stay authoritative all the way up.
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

/**
 * Slack for the END edge only. `scrollHeight`/`clientHeight` are INTEGERS (rounded from the real,
 * fractional layout box) while `scrollTop`/`scrollLeft` are fractional and snap to the DEVICE pixel
 * grid, so at a non-integer DPR the largest REACHABLE offset sits below `scrollHeight - clientHeight`
 * by up to ~1px of rounding plus ~1/DPR of snapping. Too tight an epsilon means the end edge is never
 * recognised and the wheel stays swallowed for ever — exactly the failure this guards — so 2px, which
 * covers every DPR ≥ 1. The START edge takes NO slack: 0 is both exactly representable and exactly
 * reachable, and slack there would strand the scroller's last pixels.
 */
const EDGE_SLACK_PX = 2;

/**
 * Whether a wheel of this delta has nothing left to consume on this scroller — the moment the event
 * must be left alone so it can chain to an ancestor instead of being swallowed.
 *
 * Pure and exported so the epsilon is pinned by tests: whole-module behaviour is unit-untestable
 * (jsdom reports every extent as 0, so nothing looks scrollable), which makes this predicate the
 * seam — the same choice `wheelShift.ts` and `windowEdge.ts` made.
 */
export function atScrollEdge(
  delta: number,
  offset: number,
  clientExtent: number,
  scrollExtent: number,
): boolean {
  if (delta < 0) return offset <= 0;
  if (delta > 0) return offset + clientExtent >= scrollExtent - EDGE_SLACK_PX;
  return false;
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
      // …unless it has nothing left to give in this direction. Then leave the event entirely alone:
      // the browser chains it to an ancestor itself, which is the one axis mapping it gets right.
      // Applying the clamped, no-op delta and cancelling the default is what swallowed the wheel.
      // Note the `return` — a both-axis scroller at its vertical edge must NOT fall through to the
      // sideways-panning branch below, which exists only for scrollers that cannot scroll vertically
      // at all.
      if (atScrollEdge(e.deltaY, near.el.scrollTop, near.el.clientHeight, near.el.scrollHeight)) {
        return;
      }
      near.el.scrollTop += deltaToPixels(e.deltaY, e.deltaMode, near.el.clientHeight);
      e.preventDefault();
      return;
    }
    // Nearest scroller is horizontal-only → translate the vertical wheel into sideways panning so a
    // plain mouse (no tilt/side wheel) can reach the far columns. At the horizontal edge, hand back to
    // a vertical scroller further up so the page keeps scrolling past a wide table.
    const el = near.el;
    // The handoff stays MANUAL here (see the header): the browser would chain this wheel vertically
    // on one engine and not the other, so we pick the ancestor ourselves.
    if (atScrollEdge(e.deltaY, el.scrollLeft, el.clientWidth, el.scrollWidth)) {
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

  // Horizontal wheel → drive the nearest horizontal scroller, while it still has room. At its edge
  // it is not the owner of this event either, so fall through and look further up rather than
  // swallowing it — the same latent hole as the vertical branch above, one axis over.
  if (
    near.x &&
    !atScrollEdge(e.deltaX, near.el.scrollLeft, near.el.clientWidth, near.el.scrollWidth)
  ) {
    near.el.scrollLeft += deltaToPixels(e.deltaX, e.deltaMode, near.el.clientWidth);
    e.preventDefault();
    return;
  }
  // Nearest scroller is vertical-only (or spent) → look further up for a horizontal one; otherwise
  // leave it.
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
