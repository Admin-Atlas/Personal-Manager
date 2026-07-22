// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useState, type RefObject } from "react";

/** Scroll-spy for a nested scroll container.
 *
 *  Given the scrolling element and an *ordered* list of section ids that live inside it, returns the
 *  id of the section currently at the top of the viewport — the Settings rail uses it to light the
 *  active in-tab sub-nav item. Rooted on the container, not the window.
 *
 *  It reads the live DOM on each scroll (throttled to a frame), and also when the pane resizes or a
 *  section mounts/unmounts — so a section that renders late (e.g. AI's async "Usage & cost") is picked
 *  up the moment it appears, and ids not yet in the DOM are simply skipped. The current section is the
 *  last one whose top has crossed a line just below the pane's top edge; scrolling to the very bottom
 *  always resolves to the last present section, so a short trailing section that can't reach that line
 *  still lights up. */
export function useScrollSpy(
  containerRef: RefObject<HTMLElement | null>,
  sectionIds: readonly string[],
): string | null {
  const [activeId, setActiveId] = useState<string | null>(sectionIds[0] ?? null);
  // Depend on the *set* of ids, not the array identity, so the effect re-runs on a real change
  // (tab switch) and not on every render.
  const key = sectionIds.join("|");

  useEffect(() => {
    const root = containerRef.current;
    setActiveId(sectionIds[0] ?? null); // the pane is scrolled back to the top on a tab change
    if (!root || sectionIds.length === 0) return;

    let raf = 0;
    const compute = () => {
      raf = 0;
      // A section becomes current once its top crosses this line, just below the pane's top edge.
      const line = root.getBoundingClientRect().top + 24;
      const atBottom = root.scrollTop + root.clientHeight >= root.scrollHeight - 2;
      let current: string | null = null;
      let lastPresent: string | null = null;
      for (const id of sectionIds) {
        const el = root.querySelector<HTMLElement>(`#${CSS.escape(id)}`);
        if (!el) continue; // a conditionally-rendered section that isn't in the DOM yet
        lastPresent = id;
        if (current === null) current = id; // default to the first section present
        if (el.getBoundingClientRect().top <= line) current = id; // last one past the line wins
      }
      if (atBottom && lastPresent) current = lastPresent; // the bottom always lights the last section
      if (current) setActiveId(current);
    };
    const schedule = () => {
      if (!raf) raf = requestAnimationFrame(compute);
    };

    compute();
    root.addEventListener("scroll", schedule, { passive: true });
    // Re-measure when the pane resizes or a section mounts/unmounts (e.g. the async "Usage & cost").
    const ro = new ResizeObserver(schedule);
    ro.observe(root);
    const mo = new MutationObserver(schedule);
    mo.observe(root, { childList: true });
    return () => {
      root.removeEventListener("scroll", schedule);
      ro.disconnect();
      mo.disconnect();
      if (raf) cancelAnimationFrame(raf);
    };
    // `key` stands in for the `sectionIds` array contents by design (see above).
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [containerRef, key]);

  return activeId;
}
