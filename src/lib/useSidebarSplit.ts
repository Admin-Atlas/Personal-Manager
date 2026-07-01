// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useCallback, useEffect, useRef, useState } from "react";
import { getPref, setPref } from "./ipc";

// The project sidebar's vertical split: the fraction of height given to the Milestones panel
// (top), with the project's Files filling the rest (bottom). Per board card 7E this is a *hard*
// preference — localStorage is the flash-free fast path (a synchronous read on first paint), and
// it's *also* mirrored into the encrypted `settings` table via `set_pref("project_ui", …)` so the
// ratio travels with the data folder to another machine, exactly like the theme (see ThemeContext).

const LS_KEY = "pm.project.milestonesFrac";
const PREF_KEY = "project_ui";
const DEFAULT_FRAC = 0.5; // card default: even 50-50 split
const MIN_FRAC = 0.2;
const MAX_FRAC = 0.8;

const clamp = (v: number, lo: number, hi: number) => Math.min(hi, Math.max(lo, v));

function readLs(): number | null {
  try {
    const raw = Number(localStorage.getItem(LS_KEY));
    return Number.isFinite(raw) && raw > 0 ? raw : null;
  } catch {
    return null;
  }
}
function writeLs(frac: number): void {
  try {
    localStorage.setItem(LS_KEY, String(frac));
  } catch {
    /* ignore — the split just won't persist on this device */
  }
}

/**
 * Drives the Milestones / Files split. Returns `topFrac` (the Milestones panel's height fraction,
 * for a flex-basis), a `containerRef` to attach to the split container (its height is the drag
 * basis, so the ratio stays proportional as the window resizes), a `startResize` pointer-down
 * handler for the divider, and `resizing` for cursor/select-none feedback.
 */
export function useSidebarSplit() {
  const [frac, setFrac] = useState<number>(() =>
    clamp(readLs() ?? DEFAULT_FRAC, MIN_FRAC, MAX_FRAC),
  );
  const [resizing, setResizing] = useState(false);
  const containerRef = useRef<HTMLDivElement>(null);
  const fracRef = useRef(frac);
  fracRef.current = frac;
  // localStorage empty at boot ⇒ likely a fresh machine / restored folder, so the stored mirror
  // should win on hydration. Captured once, before any write-back.
  const [bootEmpty] = useState(() => readLs() === null);

  // One-shot hydration from the settings mirror — only on a fresh machine, so a local drag is
  // never overridden. We never blind-write the mirror on mount (that would persist the default for
  // everyone); it's written only on an explicit drag-commit below.
  useEffect(() => {
    let cancelled = false;
    getPref(PREF_KEY)
      .then((raw) => {
        if (cancelled || !bootEmpty || raw == null) return;
        try {
          const blob = JSON.parse(raw) as { milestonesFrac?: unknown };
          if (typeof blob.milestonesFrac === "number") {
            const f = clamp(blob.milestonesFrac, MIN_FRAC, MAX_FRAC);
            setFrac(f);
            writeLs(f); // make the next boot flash-free
          }
        } catch {
          /* ignore a corrupt blob — keep the localStorage/default split */
        }
      })
      .catch(() => {
        /* store not ready / no value — keep the localStorage/default */
      });
    return () => {
      cancelled = true;
    };
  }, [bootEmpty]);

  const startResize = useCallback((e: React.PointerEvent) => {
    e.preventDefault();
    const container = containerRef.current;
    if (!container) return;
    const height = container.getBoundingClientRect().height;
    if (height <= 0) return;
    const startY = e.clientY;
    const startFrac = clamp(fracRef.current, MIN_FRAC, MAX_FRAC);
    setResizing(true);
    const onMove = (ev: PointerEvent) => {
      const dy = ev.clientY - startY;
      setFrac(clamp(startFrac + dy / height, MIN_FRAC, MAX_FRAC));
    };
    // Commit once on release (localStorage + the cross-machine mirror); pointercancel/blur end a
    // gesture handed off to the OS.
    const finish = () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", finish);
      window.removeEventListener("pointercancel", finish);
      window.removeEventListener("blur", finish);
      setResizing(false);
      const f = fracRef.current;
      writeLs(f);
      setPref(PREF_KEY, JSON.stringify({ milestonesFrac: f })).catch(() => {
        /* fire-and-forget — localStorage already holds the value */
      });
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", finish);
    window.addEventListener("pointercancel", finish);
    window.addEventListener("blur", finish);
  }, []);

  return { topFrac: clamp(frac, MIN_FRAC, MAX_FRAC), containerRef, startResize, resizing };
}
