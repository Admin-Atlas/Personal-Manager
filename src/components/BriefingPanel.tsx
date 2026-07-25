// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The floating daily-briefing panel: draggable by its header, resizable from its bottom-right
// corner, and mounted at app scope so it persists across every tab.
//
// "Persists across all tabs" comes free from WHERE this mounts, not from any machinery: tab routing
// is plain `useState` in App driving a ternary that unmounts the previous view, so anything rendered
// beside the view switch simply never unmounts. It also sits after all of App's vault / lock /
// onboarding early returns, so it can never leak over a locked screen.
//
// "Top-most" here means top-most WITHIN PM's window — it is a z-index rung, not an OS window. It
// sits above the docked document reader (z-40) and below dialogs (z-50), so a modal, Settings or the
// command palette still reads as blocking rather than being floated over by a briefing.
//
// Geometry lives in `lib/floatingPanel.ts` (pure + unit-tested), and every gesture routes through
// its clamp, so the panel can never be dragged under the title bar (which owns the window drag
// region and the min/max/close buttons) or off the edge of a smaller monitor.

import { useCallback, useEffect, useRef, useState } from "react";
import { Briefing } from "./Briefing";
import { useBriefing } from "../lib/briefing";
import {
  clampPanel,
  movePanel,
  readPanelRect,
  resizePanel,
  writePanelRect,
  type PanelRect,
} from "../lib/floatingPanel";
import {
  readBriefingWindow,
  subscribeBriefingPrefs,
  writeBriefingWindow,
} from "../lib/briefingPrefs";

function viewport() {
  return { w: window.innerWidth, h: window.innerHeight };
}

export function BriefingPanel() {
  // Settings renders as an overlay over the live app, so read-at-mount would leave the toggle
  // looking broken until a remount. Subscribe to the same signal the prefs writer dispatches.
  const [enabled, setEnabled] = useState(readBriefingWindow);
  useEffect(() => subscribeBriefingPrefs(() => setEnabled(readBriefingWindow())), []);

  const [rect, setRect] = useState<PanelRect>(() => readPanelRect(viewport()));
  const [dragging, setDragging] = useState(false);
  const rectRef = useRef(rect);
  rectRef.current = rect;

  // Re-clamp when the window changes size, so a panel parked against the right edge of a large
  // window doesn't end up off-screen when the window shrinks.
  useEffect(() => {
    const onResize = () => setRect((r) => clampPanel(r, viewport()));
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, []);

  // One gesture handler for both move and resize: same pointer capture, same clamp, same commit.
  // Window-level listeners (not the element's) so a fast drag that outruns the pointer keeps
  // tracking; pointercancel/blur end a gesture the OS took over, which would otherwise leave the
  // panel stuck to the cursor.
  const startGesture = useCallback((e: React.PointerEvent, mode: "move" | "resize") => {
    e.preventDefault();
    e.stopPropagation();
    const startX = e.clientX;
    const startY = e.clientY;
    const start = rectRef.current;
    setDragging(true);
    const onMove = (ev: PointerEvent) => {
      const dx = ev.clientX - startX;
      const dy = ev.clientY - startY;
      setRect(
        mode === "move"
          ? movePanel(start, dx, dy, viewport())
          : resizePanel(start, dx, dy, viewport()),
      );
    };
    const finish = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", finish);
      window.removeEventListener("pointercancel", finish);
      window.removeEventListener("blur", finish);
      setDragging(false);
      writePanelRect(rectRef.current);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", finish);
    window.addEventListener("pointercancel", finish);
    window.addEventListener("blur", finish);
  }, []);

  const { briefing, busy } = useBriefing();
  if (!enabled) return null;
  // Nothing to show and nothing being generated (an empty store): stay out of the way entirely
  // rather than floating an empty box the user then has to go and switch off.
  if (!briefing?.briefing.trim() && !busy) return null;

  return (
    <div
      role="complementary"
      aria-label="Today's briefing"
      style={{ left: rect.x, top: rect.y, width: rect.w, height: rect.h }}
      className={`fixed z-[45] flex flex-col overflow-hidden rounded-[var(--radius)] border border-border bg-panel shadow-2xl ${
        dragging ? "select-none" : ""
      }`}
    >
      {/* The header is the move handle. `touch-none` stops a touch drag scrolling the page instead. */}
      <div
        onPointerDown={(e) => startGesture(e, "move")}
        className="flex shrink-0 cursor-grab touch-none items-center justify-between gap-2 border-b border-border px-2 py-1 active:cursor-grabbing"
      >
        <span className="font-mono text-[11px] uppercase tracking-wide text-faint">Briefing</span>
        <button
          type="button"
          onPointerDown={(e) => e.stopPropagation()}
          onClick={() => {
            setEnabled(false);
            writeBriefingWindow(false);
          }}
          title="Hide the floating briefing (turn it back on in Settings → General → Focus)"
          aria-label="Hide the floating briefing"
          className="rounded-[var(--radius-sm)] px-1.5 text-xs text-ink4 hover:bg-surface hover:text-ink"
        >
          <span aria-hidden="true">✕</span>
        </button>
      </div>

      <div className="min-h-0 flex-1 overflow-y-auto px-3 py-2">
        {/* `panel` bounds its own body height; here the flex parent already scrolls, so the extra
            cap is harmless and keeps the two panel surfaces identical. */}
        <Briefing variant="panel" />
      </div>

      {/* Bottom-right resize grip. */}
      <div
        onPointerDown={(e) => startGesture(e, "resize")}
        role="separator"
        aria-label="Resize the briefing panel"
        title="Drag to resize"
        className="absolute bottom-0 right-0 h-4 w-4 cursor-se-resize touch-none"
      >
        <svg viewBox="0 0 16 16" className="h-full w-full text-ink4" aria-hidden="true">
          <path d="M15 6 L6 15 M15 11 L11 15" stroke="currentColor" strokeWidth="1.5" fill="none" />
        </svg>
      </div>
    </div>
  );
}
