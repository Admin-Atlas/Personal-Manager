// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Pinboard's state + persistence. Board layout is arrangement state, so it lives in
// localStorage (the same seam as the theme in ThemeContext) — non-secret, reads synchronously
// for a flicker-free first paint, and the read/write here can later move to IPC without
// touching the view. localStorage can throw in locked-down webviews, so every access is guarded.

import { useCallback, useEffect, useState } from "react";
import { clampRect, findFreeRect } from "./grid";
import {
  BOARD_VERSION,
  EMPTY_BOARD,
  type Board,
  type Rect,
  type TimelineItem,
  type Widget,
} from "./types";

const STORAGE_KEY = "pm:pinboard";

/** A stable unique id; `crypto.randomUUID` in the webview, with a cheap fallback. */
function makeId(): string {
  try {
    return crypto.randomUUID();
  } catch {
    return `w-${Date.now().toString(36)}-${Math.floor(Math.random() * 1e6).toString(36)}`;
  }
}

/** A widget is only usable if it has an id, a known kind, and a fully-numeric rect — the
 *  view dereferences `w.rect.x` unconditionally while rendering, and there's no error
 *  boundary, so one malformed entry would white-screen the app. Validate per-widget, not
 *  just the top-level shape. */
function isValidWidget(w: unknown): w is Widget {
  if (!w || typeof w !== "object") return false;
  const x = w as Record<string, unknown>;
  if (typeof x.id !== "string" || (x.kind !== "note" && x.kind !== "timeline")) return false;
  const r = x.rect as Record<string, unknown> | undefined;
  return (
    !!r &&
    typeof r.x === "number" &&
    typeof r.y === "number" &&
    typeof r.w === "number" &&
    typeof r.h === "number"
  );
}

function loadBoard(): Board {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return EMPTY_BOARD;
    const parsed = JSON.parse(raw) as Board;
    // Discard anything that isn't the current shape rather than rendering garbage — both the
    // envelope and each widget, clamping rects back in-bounds for safety.
    if (!parsed || parsed.version !== BOARD_VERSION || !Array.isArray(parsed.widgets)) {
      return EMPTY_BOARD;
    }
    const widgets = parsed.widgets
      .filter(isValidWidget)
      .map((w) => ({ ...w, rect: clampRect(w.rect) }));
    return { version: BOARD_VERSION, widgets };
  } catch {
    return EMPTY_BOARD;
  }
}

function saveBoard(board: Board): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(board));
  } catch {
    /* ignore — the board just won't persist */
  }
}

/** Board state + the mutators the view needs. Every change re-persists synchronously. */
export function usePinboard() {
  const [board, setBoard] = useState<Board>(loadBoard);

  useEffect(() => {
    saveBoard(board);
  }, [board]);

  const addNote = useCallback(() => {
    setBoard((b) => {
      const rect = findFreeRect(b.widgets, 7, 5);
      const widget: Widget = { id: makeId(), kind: "note", rect, text: "", color: "st-quick" };
      return { ...b, widgets: [...b.widgets, widget] };
    });
  }, []);

  const addTimeline = useCallback(() => {
    setBoard((b) => {
      const rect = findFreeRect(b.widgets, 9, 8);
      const widget: Widget = { id: makeId(), kind: "timeline", rect, title: "Timeline", items: [] };
      return { ...b, widgets: [...b.widgets, widget] };
    });
  }, []);

  const updateWidget = useCallback((id: string, patch: Partial<Widget>) => {
    setBoard((b) => ({
      ...b,
      widgets: b.widgets.map((w) => (w.id === id ? { ...w, ...patch } : w)),
    }));
  }, []);

  const moveWidget = useCallback((id: string, rect: Rect) => {
    setBoard((b) => ({
      ...b,
      widgets: b.widgets.map((w) => (w.id === id ? { ...w, rect } : w)),
    }));
  }, []);

  const removeWidget = useCallback((id: string) => {
    setBoard((b) => ({ ...b, widgets: b.widgets.filter((w) => w.id !== id) }));
  }, []);

  /** Move a widget to the end of the list (= painted on top) when it's interacted with. */
  const raiseWidget = useCallback((id: string) => {
    setBoard((b) => {
      if (b.widgets[b.widgets.length - 1]?.id === id) return b;
      const w = b.widgets.find((x) => x.id === id);
      if (!w) return b;
      return { ...b, widgets: [...b.widgets.filter((x) => x.id !== id), w] };
    });
  }, []);

  const addTimelineItem = useCallback((id: string) => {
    const item: TimelineItem = { id: makeId(), date: "", label: "" };
    setBoard((b) => ({
      ...b,
      widgets: b.widgets.map((w) =>
        w.id === id ? { ...w, items: [...(w.items ?? []), item] } : w,
      ),
    }));
  }, []);

  const updateTimelineItem = useCallback(
    (id: string, itemId: string, patch: Partial<TimelineItem>) => {
      setBoard((b) => ({
        ...b,
        widgets: b.widgets.map((w) =>
          w.id === id
            ? { ...w, items: (w.items ?? []).map((it) => (it.id === itemId ? { ...it, ...patch } : it)) }
            : w,
        ),
      }));
    },
    [],
  );

  const removeTimelineItem = useCallback((id: string, itemId: string) => {
    setBoard((b) => ({
      ...b,
      widgets: b.widgets.map((w) =>
        w.id === id ? { ...w, items: (w.items ?? []).filter((it) => it.id !== itemId) } : w,
      ),
    }));
  }, []);

  return {
    board,
    addNote,
    addTimeline,
    updateWidget,
    moveWidget,
    removeWidget,
    raiseWidget,
    addTimelineItem,
    updateTimelineItem,
    removeTimelineItem,
  };
}
