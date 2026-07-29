// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Which edge of a frameless window a point sits on. Pure geometry, kept apart from the hook that
// uses it (components/ui/useEdgeResizeCursor.ts) so the hit test can be reasoned about and tested
// without a window, a webview, or a platform.
//
// This deliberately mirrors `hit_test` in tauri-runtime-wry's `undecorated_resizing`: PM draws no
// resize border of its own, so the cursor only tells the truth if it matches where the native
// handler will actually call `begin_resize_drag`.

/** The eight resizable edges, or null for the window interior. */
export type WindowEdge = "n" | "s" | "e" | "w" | "ne" | "nw" | "se" | "sw" | null;

/**
 * Which edge `(x, y)` sits on within a `width` x `height` window, given a `band` inset in the same
 * units. Corners take precedence over the sides that form them, matching the native hit test.
 *
 * The band is inclusive: a point exactly `band` from an edge is still on it, since the native
 * comparison is `<=`.
 */
export function edgeAt(
  x: number,
  y: number,
  width: number,
  height: number,
  band: number,
): WindowEdge {
  const nearW = x <= band;
  const nearE = x >= width - band;
  const nearN = y <= band;
  const nearS = y >= height - band;
  if (nearN && nearW) return "nw";
  if (nearN && nearE) return "ne";
  if (nearS && nearW) return "sw";
  if (nearS && nearE) return "se";
  if (nearN) return "n";
  if (nearS) return "s";
  if (nearW) return "w";
  if (nearE) return "e";
  return null;
}
