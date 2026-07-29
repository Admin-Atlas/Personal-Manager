// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Linux-only: paint a resize cursor over the frameless window's edges.
//
// PM runs undecorated (`decorations: false`), so Tauri synthesises the resize border itself. On
// Windows that border is a transparent HWND answering WM_NCHITTEST, and the OS supplies the cursor
// for free; on macOS AppKit owns it. On Linux, `tauri-runtime-wry`'s GTK path connects only
// `connect_button_press_event` and `connect_touch_event` — it calls `begin_resize_drag` on press and
// NEVER sets a cursor. tao carries the same gap as an acknowledged FIXME ("calling
// begin_resize_drag uses the default cursor, it should show a resizing cursor instead"), and its own
// hover handler sets the cursor on the toplevel GdkWindow, which the webview child masks. So
// dragging an edge works, but nothing tells you an edge is there.
//
// The fix is CSS, deliberately: WebKit paints the cursor from its own hit test, which is the only
// layer that can win over the webview child. We only move a class on <html>; the resize itself keeps
// being performed by the native GTK press handler that already works. No DOM overlay, so nothing can
// steal a click from the scrollbar thumb, the caption buttons, or a collapse tab.
//
// Deliberately mirrors the native hit test rather than approximating it, since a cursor that appears
// where `begin_resize_drag` will NOT fire is worse than no cursor at all:
//   - band width `BORDERLESS_RESIZE_INSET * scale_factor` (5 logical px x the GTK scale, which the
//     webview sees as devicePixelRatio)
//   - suppressed while maximized — the GTK handler bails on `is_maximized()`, so PM launches
//     (`"maximized": true`) with no resize available at all until the window is restored.

import { useEffect } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { edgeAt, type WindowEdge } from "../../lib/windowEdge";

/** Matches `BORDERLESS_RESIZE_INSET` in tauri-runtime-wry's `undecorated_resizing`. */
const RESIZE_INSET = 5;

/** WebKitGTK is the Linux webview; Blink (Windows) and WebKit/AppKit (macOS) get the cursor from
 *  the OS. Checked via the platform string rather than the engine, because the gap is in the GTK
 *  windowing path, not in WebKit itself. */
function isLinux(): boolean {
  if (typeof navigator === "undefined") return false;
  return /Linux|X11/.test(navigator.userAgent) && !/Android/.test(navigator.userAgent);
}

const EDGE_CLASSES = [
  "pm-rz-n",
  "pm-rz-s",
  "pm-rz-e",
  "pm-rz-w",
  "pm-rz-ne",
  "pm-rz-nw",
  "pm-rz-se",
  "pm-rz-sw",
];

/** Paint a resize cursor over the window edges on Linux. A no-op everywhere else. */
export function useEdgeResizeCursor(): void {
  useEffect(() => {
    if (!isLinux()) return;

    const root = document.documentElement;
    let current: WindowEdge = null;
    // The GTK handler refuses to resize a maximized window, so the cursor must not offer it.
    // Assume maximized until proven otherwise: PM launches maximized, and promising a drag that
    // does nothing is the worse failure.
    let maximized = true;
    let frame = 0;

    const apply = (next: WindowEdge) => {
      if (next === current) return;
      current = next;
      root.classList.remove(...EDGE_CLASSES);
      if (next) root.classList.add(`pm-rz-${next}`);
    };

    const onMove = (e: MouseEvent) => {
      // Coalesce to one update per frame: mousemove fires far faster than the compositor, and this
      // runs on every pointer move in the app.
      if (frame) return;
      frame = requestAnimationFrame(() => {
        frame = 0;
        if (maximized) {
          apply(null);
          return;
        }
        const band = RESIZE_INSET * (window.devicePixelRatio || 1);
        apply(edgeAt(e.clientX, e.clientY, window.innerWidth, window.innerHeight, band));
      });
    };

    // Leaving the window entirely (or a drag starting) must not strand the cursor class.
    const onLeave = () => apply(null);

    const win = getCurrentWindow();
    let unlisten: (() => void) | undefined;
    const syncMaximized = () => {
      win
        .isMaximized()
        .then((m) => {
          maximized = m;
          if (m) apply(null);
        })
        .catch(() => {});
    };
    syncMaximized();
    win
      .onResized(syncMaximized)
      .then((fn) => {
        unlisten = fn;
      })
      .catch(() => {});

    window.addEventListener("mousemove", onMove, { passive: true });
    document.addEventListener("mouseleave", onLeave);
    return () => {
      if (frame) cancelAnimationFrame(frame);
      window.removeEventListener("mousemove", onMove);
      document.removeEventListener("mouseleave", onLeave);
      root.classList.remove(...EDGE_CLASSES);
      unlisten?.();
    };
  }, []);
}
