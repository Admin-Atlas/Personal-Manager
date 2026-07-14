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
import { clampRect, COLS, dissolveFolders, findFreeRect, minSize, resolveDrop, ROWS } from "./grid";
import { DEFAULT_TINT } from "./palette";
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
  if (typeof x.id !== "string") return false;
  if (x.kind !== "note" && x.kind !== "timeline" && x.kind !== "folder") return false;
  const r = x.rect as Record<string, unknown> | undefined;
  const rectOk =
    !!r &&
    typeof r.x === "number" &&
    typeof r.y === "number" &&
    typeof r.w === "number" &&
    typeof r.h === "number";
  if (!rectOk) return false;
  if (x.kind === "folder") {
    // Children must be an array of valid, NON-folder widgets (folders never nest). A single
    // malformed child fails the whole folder rather than white-screening the app on render —
    // there's no error boundary, and the view dereferences child fields unconditionally.
    return (
      Array.isArray(x.children) &&
      x.children.every((c) => isValidWidget(c) && (c as Widget).kind !== "folder")
    );
  }
  return true;
}

/** Apply `fn` to the widget with `id` wherever it lives — top-level or a folder child. Folders
 *  never nest, so a depth-1 walk reaches everything, and note/timeline mutators keep working
 *  unchanged whether the target is on the board or inside an open folder. */
function mapWidget(widgets: Widget[], id: string, fn: (w: Widget) => Widget): Widget[] {
  return widgets.map((w) => {
    if (w.id === id) return fn(w);
    if (w.kind === "folder" && w.children?.some((c) => c.id === id)) {
      return { ...w, children: w.children.map((c) => (c.id === id ? fn(c) : c)) };
    }
    return w;
  });
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
    const widgets = valid.map((w) => {
      const rect = clampRect(w.rect, boundCols, boundRows, minSize(w.kind));
      if (w.kind === "folder") {
        // Clamp children too (kind-aware), so a folder authored elsewhere renders cleanly.
        const children = (w.children ?? []).map((c) => ({
          ...c,
          rect: clampRect(c.rect, boundCols, boundRows, minSize(c.kind)),
        }));
        return { ...w, rect, children };
      }
      return { ...w, rect };
    });
    // Heal any folder a corrupt/hand-edited pref left with ≤1 child.
    return { version: BOARD_VERSION, widgets: dissolveFolders(widgets, boundCols, boundRows) };
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

  // add* return the new widget's id so the view can scroll it into sight — a note placed on a
  // lower row would otherwise be created off-screen. The id is minted up front (outside the
  // functional update) so it can be returned synchronously.
  const addNote = useCallback(() => {
    const id = makeId();
    setBoard((b) => {
      const rect = findFreeRect(b.widgets, 7, 5, boundsRef.current.cols, boundsRef.current.rows);
      const widget: Widget = { id, kind: "note", rect, text: "", color: DEFAULT_TINT };
      return { ...b, widgets: [...b.widgets, widget] };
    });
    return id;
  }, []);

  const addTimeline = useCallback(() => {
    const id = makeId();
    setBoard((b) => {
      const rect = findFreeRect(b.widgets, 9, 8, boundsRef.current.cols, boundsRef.current.rows);
      // Seed an empty title so the header shows its "Timeline" placeholder (matching notes/folders).
      const widget: Widget = { id, kind: "timeline", rect, title: "", items: [] };
      return { ...b, widgets: [...b.widgets, widget] };
    });
    return id;
  }, []);

  const updateWidget = useCallback((id: string, patch: Partial<Widget>) => {
    setBoard((b) => ({ ...b, widgets: mapWidget(b.widgets, id, (w) => ({ ...w, ...patch })) }));
  }, []);

  /** Reposition a widget (used by resize; move gestures go through {@link dropWidget}). */
  const moveWidget = useCallback((id: string, rect: Rect) => {
    setBoard((b) => ({
      ...b,
      widgets: b.widgets.map((w) => (w.id === id ? { ...w, rect } : w)),
    }));
  }, []);

  /** Commit a MOVE drop: may merge two identically-placed widgets into a folder, drop a widget into
   *  an overlapped folder, or just reposition (see {@link resolveDrop}). */
  const dropWidget = useCallback((id: string, rect: Rect) => {
    setBoard((b) => ({
      ...b,
      widgets: resolveDrop(
        b.widgets,
        id,
        rect,
        boundsRef.current.cols,
        boundsRef.current.rows,
        makeId,
      ),
    }));
  }, []);

  const removeWidget = useCallback((id: string) => {
    setBoard((b) => {
      const top = b.widgets.find((w) => w.id === id);
      if (top) {
        // A top-level FOLDER ungroups (spill its children back onto the board — non-destructive, so
        // deleting a tile never nukes the user's notes); a note/timeline is deleted outright.
        if (top.kind === "folder") {
          const { cols, rows } = boundsRef.current;
          let others = b.widgets.filter((w) => w.id !== id);
          for (const child of top.children ?? []) {
            const rect = findFreeRect(others, child.rect.w, child.rect.h, cols, rows);
            others = [...others, { ...child, rect }];
          }
          return { ...b, widgets: others };
        }
        return { ...b, widgets: b.widgets.filter((w) => w.id !== id) };
      }
      // A folder CHILD: remove it from its parent, then auto-dissolve if the folder is now ≤1.
      const widgets = b.widgets.map((w) =>
        w.kind === "folder" && w.children?.some((c) => c.id === id)
          ? { ...w, children: (w.children ?? []).filter((c) => c.id !== id) }
          : w,
      );
      return {
        ...b,
        widgets: dissolveFolders(widgets, boundsRef.current.cols, boundsRef.current.rows),
      };
    });
  }, []);

  /** Pull a child out of a folder back onto the board — at `rect` when dragged there, else a free
   *  slot. Releasing it back over a folder re-files it; the source folder auto-dissolves if drained. */
  const popOutChild = useCallback((folderId: string, childId: string, rect?: Rect) => {
    setBoard((b) => {
      const { cols, rows } = boundsRef.current;
      const folder = b.widgets.find((w) => w.id === folderId && w.kind === "folder");
      const child = folder?.children?.find((c) => c.id === childId);
      if (!folder || !child) return b;
      const remaining = (folder.children ?? []).filter((c) => c.id !== childId);
      const landing = rect
        ? clampRect(rect, cols, rows, minSize(child.kind))
        : findFreeRect(b.widgets, child.rect.w, child.rect.h, cols, rows);
      let ws = b.widgets.map((w) => (w.id === folderId ? { ...w, children: remaining } : w));
      ws = [...ws, { ...child, rect: landing }];
      ws = resolveDrop(ws, child.id, landing, cols, rows, makeId); // dropped onto a folder → re-file
      ws = dissolveFolders(ws, cols, rows); // source folder may now be ≤1 child
      return { ...b, widgets: ws };
    });
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
      widgets: mapWidget(b.widgets, id, (w) => ({ ...w, items: [...(w.items ?? []), item] })),
    }));
  }, []);

  const updateTimelineItem = useCallback(
    (id: string, itemId: string, patch: Partial<TimelineItem>) => {
      setBoard((b) => ({
        ...b,
        widgets: mapWidget(b.widgets, id, (w) => ({
          ...w,
          items: (w.items ?? []).map((it) => (it.id === itemId ? { ...it, ...patch } : it)),
        })),
      }));
    },
    [],
  );

  const removeTimelineItem = useCallback((id: string, itemId: string) => {
    setBoard((b) => ({
      ...b,
      widgets: mapWidget(b.widgets, id, (w) => ({
        ...w,
        items: (w.items ?? []).filter((it) => it.id !== itemId),
      })),
    }));
  }, []);

  return {
    board,
    addNote,
    addTimeline,
    updateWidget,
    moveWidget,
    dropWidget,
    removeWidget,
    raiseWidget,
    popOutChild,
    addTimelineItem,
    updateTimelineItem,
    removeTimelineItem,
  };
}
