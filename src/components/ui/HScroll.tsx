// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useRef, type ReactNode } from "react";

/**
 * A horizontally-scrollable container that also pans on a **plain vertical mouse wheel** — so a mouse
 * with no tilt/side-scroll wheel can still move a wide table or grid sideways (the OS/browser only
 * maps the wheel to horizontal scroll on a shift-wheel or a tilt wheel otherwise).
 *
 * It's deliberately polite: it only hijacks the wheel when this element actually overflows
 * horizontally, has no vertical scroll of its own to honour, shift isn't held, and there's still room
 * to pan in the wheel's direction — at the horizontal edge it lets the event through so the page
 * keeps scrolling vertically. Wrap any wide table/grid in this instead of a bare `overflow-x-auto`.
 */
export function HScroll({ children, className }: { children: ReactNode; className?: string }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (e.deltaY === 0 || e.shiftKey) return; // tilt/shift already scroll horizontally
      if (el.scrollWidth <= el.clientWidth) return; // nothing to pan
      if (el.scrollHeight > el.clientHeight + 1) return; // it scrolls vertically itself — leave it
      const atStart = el.scrollLeft <= 0;
      const atEnd = el.scrollLeft + el.clientWidth >= el.scrollWidth - 1;
      if ((e.deltaY < 0 && atStart) || (e.deltaY > 0 && atEnd)) return; // at the edge → let page scroll
      el.scrollLeft += e.deltaY;
      e.preventDefault();
    };
    // Passive must be false so preventDefault can stop the page from scrolling while we pan.
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, []);
  return (
    <div ref={ref} className={`overflow-x-auto ${className ?? ""}`}>
      {children}
    </div>
  );
}
