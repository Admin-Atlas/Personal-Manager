// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Pinboard's state + persistence. The board is user content (notes, timelines), so it
// lives in the encrypted `settings` table via `get_pref`/`set_pref` — it travels with the data
// folder on backup/transfer and is encrypted at rest, rather than sitting in the webview's
// localStorage. It already loaded asynchronously on mount, so reading it over IPC is no
// regression; the persist effect is gated on the initial load so the empty default can't
// clobber a stored board before it arrives.

import { useCallback, useEffect, useRef, useState } from "react";
import { getPref, setPref } from "../ipc";
import { clampRect, COLS, findFreeRect, ROWS } from "./grid";
import {
  BOARD_VERSION,
  EMPTY_BOARD,
  type Board,
  type Rect,
  type TimelineItem,
  type Widget,
} from "./types";

// The settings-table key (must match the backend `set_pref` allowlist).
const PREF_KEY = "pinboard";

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

/** A generous absolute cap (cells) so a corrupt/hand-edited pref can't blow the board up to a
 *  pathological size, while still dwarfing any real (even multi-monitor) layout. */
const MAX_BOARD_CELLS = 512;

function parseBoard(raw: string | null, cols: number, rows: number): Board {
  if (!raw) return EMPTY_BOARD;
  try {
    const parsed = JSON.parse(raw) as Board;
    // Discard anything that isn't the current shape rather than rendering garbage — both the
    // envelope and each widget, clamping rects back in-bounds for safety.
    if (!parsed || parsed.version !== BOARD_VERSION || !Array.isArray(parsed.widgets)) {
      return EMPTY_BOARD;
    }
    const valid = parsed.widgets.filter(isValidWidget);
    // Expand the clamp bounds to contain every already-valid widget (capped), so a board authored on
    // a larger screen isn't snapped inward when reopened on a smaller one — the board just scrolls to
    // reach it. A single widget beyond the sane cap is still clamped, guarding a corrupt pref.
    let boundCols = cols;
    let boundRows = rows;
    for (const w of valid) {
      boundCols = Math.max(boundCols, Math.min(w.rect.x + w.rect.w, MAX_BOARD_CELLS));
      boundRows = Math.max(boundRows, Math.min(w.rect.y + w.rect.h, MAX_BOARD_CELLS));
    }
    const widgets = valid.map((w) => ({ ...w, rect: clampRect(w.rect, boundCols, boundRows) }));
    return { version: BOARD_VERSION, widgets };
  } catch {
    return EMPTY_BOARD;
  }
}

/** Board state + the mutators the view needs. Loads once from the store on mount and
 *  re-persists every change. `bounds` is the board's maximum cell extent (the screen size —
 *  see PinboardView): stored widgets are clamped to it on load and new widgets placed within it,
 *  so a widget dragged into the enlarged board survives a reload instead of snapping back. */
export function usePinboard(bounds: { cols: number; rows: number } = { cols: COLS, rows: ROWS }) {
  const [board, setBoard] = useState<Board>(EMPTY_BOARD);
  const loaded = useRef(false);
  // Read through a ref so the mount-only load effect and the stable mutators always see the
  // current bounds without re-subscribing (bounds is derived from the fixed screen size, so it
  // doesn't actually change, but this keeps the hooks honest).
  const boundsRef = useRef(bounds);
  boundsRef.current = bounds;

  // Load the stored board once. Until this resolves, `loaded` stays false so the persist
  // effect below won't write the empty default over a real board.
  useEffect(() => {
    let cancelled = false;
    getPref(PREF_KEY)
      .then((raw) => {
        if (!cancelled) setBoard(parseBoard(raw, boundsRef.current.cols, boundsRef.current.rows));
      })
      .catch(() => {
        /* store not ready — keep the empty board */
      })
      .finally(() => {
        loaded.current = true;
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // Persist the board — DEBOUNCED (F-15). The board object changes on every keystroke (a note's text
  // lives in it), and writing the whole JSON to the encrypted store each time is one IPC + SQLCipher
  // write per character. Coalesce to a single write ~500 ms after the last change; a `boardRef` keeps
  // the latest value so the trailing timer (and the unmount flush below) writes what's current.
  const boardRef = useRef(board);
  boardRef.current = board;
  useEffect(() => {
    if (!loaded.current) return;
    const handle = setTimeout(() => {
      setPref(PREF_KEY, JSON.stringify(boardRef.current)).catch(() => {
        /* ignore — the board just won't persist this change */
      });
    }, 500);
    return () => clearTimeout(handle);
  }, [board]);
  // Flush the latest board on unmount so a fast navigate-away/close doesn't drop the tail edit.
  useEffect(
    () => () => {
      if (loaded.current) {
        setPref(PREF_KEY, JSON.stringify(boardRef.current)).catch(() => {});
      }
    },
    [],
  );

  const addNote = useCallback(() => {
    setBoard((b) => {
      const rect = findFreeRect(b.widgets, 7, 5, boundsRef.current.cols, boundsRef.current.rows);
      const widget: Widget = { id: makeId(), kind: "note", rect, text: "", color: "st-quick" };
      return { ...b, widgets: [...b.widgets, widget] };
    });
  }, []);

  const addTimeline = useCallback(() => {
    setBoard((b) => {
      const rect = findFreeRect(b.widgets, 9, 8, boundsRef.current.cols, boundsRef.current.rows);
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
            ? {
                ...w,
                items: (w.items ?? []).map((it) => (it.id === itemId ? { ...it, ...patch } : it)),
              }
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
