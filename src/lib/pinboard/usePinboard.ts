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
import {
  clampRect,
  COLS,
  findFreeRect,
  FOLDER_H,
  FOLDER_W,
  minSize,
  reflowToWidth,
  resolveDrop,
  ROWS,
} from "./grid";
import {
  canRedo,
  canUndo,
  commit,
  commitBarrier,
  commitSilent,
  initHistory,
  redo,
  resetHistory,
  undo,
  type History,
} from "./history";
import { DEFAULT_TINT } from "./palette";
import {
  BOARD_VERSION,
  EMPTY_BOARD,
  PINBOARD_PREF_KEY,
  type Board,
  type CellPoint,
  type Rect,
  type TimelineItem,
  type Widget,
} from "./types";

// The settings-table key (must match the backend `set_pref` allowlist).
const PREF_KEY = PINBOARD_PREF_KEY;

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
    // The board is a FIXED-WIDTH canvas = `cols` (the window) and grows only downward; give it a
    // generous vertical cap so widgets that overhang the width wrap onto new rows (the board scrolls)
    // rather than piling up. `rows` is the caller's generous vertical extent (≈ the screen height).
    const rowCap = Math.min(Math.max(rows, ROWS), MAX_BOARD_CELLS);
    // Sanity-clamp every rect to a generous box first (preserving the stored x/y up to the cap, so a
    // fitter keeps its exact spot) — this only guards a corrupt/hand-edited pref. Children are
    // clamped kind-aware; they aren't board-positioned so they don't take part in the re-flow.
    const sane = valid.map((w) => {
      const rect = clampRect(w.rect, MAX_BOARD_CELLS, MAX_BOARD_CELLS, minSize(w.kind));
      if (w.kind === "folder") {
        const children = (w.children ?? []).map((c) => ({
          ...c,
          rect: clampRect(c.rect, MAX_BOARD_CELLS, MAX_BOARD_CELLS, minSize(c.kind)),
        }));
        return { ...w, rect, children };
      }
      return { ...w, rect };
    });
    // Re-flow any widget that overhangs the fixed width back on-screen (wrapping to a new row). A
    // folder is NOT normalised by child count: an empty or single-card folder is a legitimate thing
    // the user made (+ Folder) and must survive a reload.
    return { version: BOARD_VERSION, widgets: reflowToWidth(sane, cols, rowCap) };
  } catch {
    return EMPTY_BOARD;
  }
}

/** How a change enters the undo history. See {@link commitForPatch} and the mutators below for which
 *  change is which, and history.ts for why `silent` and `barrier` exist at all. */
type CommitKind = { mode: "push"; key?: string | null } | { mode: "silent" } | { mode: "barrier" };

const SILENT: CommitKind = { mode: "silent" };

/** The undoable weight of a board: the text it holds. Snapshots share structure for everything the
 *  user didn't touch, so rects and ids are effectively free — but a note's text is a fresh string on
 *  every keystroke, and it is the only thing here that can grow without bound. */
function weighBoard(b: Board): number {
  let n = 0;
  for (const w of b.widgets) {
    n += w.text?.length ?? 0;
    for (const c of w.children ?? []) n += c.text?.length ?? 0;
  }
  return n;
}

/** How a widget patch should be recorded. The default is one undo step per change; the exceptions are
 *  the changes that reach past the board. */
function commitForPatch(id: string, patch: Partial<Widget>): CommitKind {
  // Ingest metadata mirrors a REAL vault document, which undo cannot delete. Rolling these fields
  // back would only make the note lie about whether it had been filed.
  if ("ingestedAt" in patch || "ingestedHash" in patch) return { mode: "silent" };
  // Linking a timeline to a project writes real milestones to the backend BEFORE this patch lands.
  // No board snapshot can retract them, and restoring the freeform entries they were made from would
  // draw both on the calendar — so a link is where the board's history honestly ends. (Unlinking
  // writes nothing, so it stays an ordinary, undoable step.)
  if ("project" in patch && patch.project !== undefined) return { mode: "barrier" };
  // Typing is grouped so one Ctrl+Z doesn't take a single character — keyed per widget and per field
  // so two notes, or a title and a body, never merge into each other.
  if ("text" in patch) return { mode: "push", key: `text:${id}` };
  if ("title" in patch) return { mode: "push", key: `title:${id}` };
  // Everything else — colour, geometry, view toggles — is its own step. Three colour clicks should be
  // three undos; and the swatches only show while editing, so a shared key would let a colour change
  // be swallowed by the text bucket around it.
  return { mode: "push" };
}

/** Board state + the mutators the view needs. Loads once from the store on mount and
 *  re-persists every change. `bounds` is the board's maximum cell extent (the screen size —
 *  see PinboardView): stored widgets are clamped to it on load and new widgets placed within it,
 *  so a widget dragged into the enlarged board survives a reload instead of snapping back.
 *
 *  Undo lives HERE rather than around the view's handlers, because this is the only writer: widgets
 *  are also created and destroyed inside `resolveDrop` (filing, folding) and dropped by `parseBoard`,
 *  paths no caller sees — and nothing outside could rebuild a deleted widget's id, text and rect
 *  anyway. One funnel, one place to be right. */
export function usePinboard(bounds: { cols: number; rows: number } = { cols: COLS, rows: ROWS }) {
  const [hist, setHist] = useState<History<Board>>(() => initHistory(EMPTY_BOARD));
  const board = hist.present;
  const loaded = useRef(false);
  // Read through a ref so the mount-only load effect and the stable mutators always see the
  // current bounds without re-subscribing (bounds is derived from the fixed screen size, so it
  // doesn't actually change, but this keeps the hooks honest).
  const boundsRef = useRef(bounds);
  boundsRef.current = bounds;

  // Load the stored board once. Until this resolves, `loaded` stays false so the persist
  // effect below won't write the empty default over a real board.
  const [ready, setReady] = useState(false);
  useEffect(() => {
    let cancelled = false;
    getPref(PREF_KEY)
      .then((raw) => {
        // RESET, not commit: the board didn't change, it arrived. Committing it would leave the empty
        // default sitting in `past`, and the very first Ctrl+Z would wipe the user's board.
        if (!cancelled) {
          setHist(resetHistory(parseBoard(raw, boundsRef.current.cols, boundsRef.current.rows)));
        }
      })
      .catch(() => {
        /* store not ready — keep the empty board (history already holds exactly that) */
      })
      .finally(() => {
        loaded.current = true;
        if (!cancelled) setReady(true);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  /** The one writer. `fn` produces the next board from the current one; `kind` says how that enters
   *  the undo history. `now` is read out here so the state updater stays pure (React may run it
   *  twice under StrictMode). */
  const change = useCallback((fn: (b: Board) => Board, kind: CommitKind = { mode: "push" }) => {
    const now = Date.now();
    setHist((h) => {
      const next = fn(h.present);
      if (kind.mode === "silent") return commitSilent(h, next);
      if (kind.mode === "barrier") return commitBarrier(h, next);
      return commit(h, next, { key: kind.key ?? null, now, weigh: weighBoard });
    });
  }, []);

  const undoBoard = useCallback(() => setHist((h) => undo(h)), []);
  const redoBoard = useCallback(() => setHist((h) => redo(h)), []);

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
    change((b) => {
      const rect = findFreeRect(b.widgets, 7, 5, boundsRef.current.cols, boundsRef.current.rows);
      const widget: Widget = { id, kind: "note", rect, text: "", color: DEFAULT_TINT };
      return { ...b, widgets: [...b.widgets, widget] };
    });
    return id;
  }, [change]);

  const addTimeline = useCallback(() => {
    const id = makeId();
    change((b) => {
      const rect = findFreeRect(b.widgets, 9, 8, boundsRef.current.cols, boundsRef.current.rows);
      // Seed an empty title so the header shows its "Timeline" placeholder (matching notes/folders).
      const widget: Widget = { id, kind: "timeline", rect, title: "", items: [] };
      return { ...b, widgets: [...b.widgets, widget] };
    });
    return id;
  }, [change]);

  /** An EMPTY folder, made on purpose. Nothing auto-dissolves it: it lives until the user ungroups
   *  it, so it can sit there waiting to be filled. `children: []` is not optional — `isValidWidget`
   *  silently drops a folder whose `children` isn't an array, which would lose it on the next load. */
  const addFolder = useCallback(() => {
    const id = makeId();
    change((b) => {
      const rect = findFreeRect(
        b.widgets,
        FOLDER_W,
        FOLDER_H,
        boundsRef.current.cols,
        boundsRef.current.rows,
      );
      const widget: Widget = {
        id,
        kind: "folder",
        rect,
        title: "",
        children: [],
        expandMode: "inline",
      };
      return { ...b, widgets: [...b.widgets, widget] };
    });
    return id;
  }, [change]);

  const updateWidget = useCallback(
    (id: string, patch: Partial<Widget>) => {
      change(
        (b) => ({ ...b, widgets: mapWidget(b.widgets, id, (w) => ({ ...w, ...patch })) }),
        commitForPatch(id, patch),
      );
    },
    [change],
  );

  /** Reposition a widget (used by resize; move gestures go through {@link dropWidget}). */
  const moveWidget = useCallback(
    (id: string, rect: Rect) => {
      change((b) => ({
        ...b,
        widgets: b.widgets.map((w) => (w.id === id ? { ...w, rect } : w)),
      }));
    },
    [change],
  );

  /** Commit a MOVE drop: may file the widget into the folder under the POINTER, merge two
   *  identically-placed widgets into a folder, or just reposition (see {@link resolveDrop}).
   *  `pointer` is the board cell the mouse was over on release, and is required rather than
   *  optional so every caller has to say what it means — `null` (unknown) files into nothing,
   *  which is the safe reading. */
  const dropWidget = useCallback(
    (id: string, rect: Rect, pointer: CellPoint | null) => {
      change((b) => ({
        ...b,
        widgets: resolveDrop(
          b.widgets,
          id,
          rect,
          boundsRef.current.cols,
          boundsRef.current.rows,
          makeId,
          pointer,
        ),
      }));
    },
    [change],
  );

  const removeWidget = useCallback(
    (id: string) => {
      change((b) => {
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
        // A folder CHILD: remove it from its parent. The folder stays, however few cards are left —
        // it's the user's object, and its ✕ ("Ungroup") is how it goes away.
        return {
          ...b,
          widgets: b.widgets.map((w) =>
            w.kind === "folder" && w.children?.some((c) => c.id === id)
              ? { ...w, children: (w.children ?? []).filter((c) => c.id !== id) }
              : w,
          ),
        };
      });
    },
    [change],
  );

  /** Pull a child out of a folder back onto the board, into a free slot. The source folder stays put
   *  (empty if that was its last card) — only the user's ✕ ungroups it. */
  const popOutChild = useCallback(
    (folderId: string, childId: string) => {
      change((b) => {
        const { cols, rows } = boundsRef.current;
        const folder = b.widgets.find((w) => w.id === folderId && w.kind === "folder");
        const child = folder?.children?.find((c) => c.id === childId);
        if (!folder || !child) return b;
        const remaining = (folder.children ?? []).filter((c) => c.id !== childId);
        // A free slot, so the popped card lands in the clear. It is NOT put back through resolveDrop:
        // findFreeRect falls back to an OVERLAPPING origin when the board is full, which would let a
        // card pop straight back into the folder it just came out of.
        const landing = findFreeRect(b.widgets, child.rect.w, child.rect.h, cols, rows);
        const ws = b.widgets.map((w) => (w.id === folderId ? { ...w, children: remaining } : w));
        return { ...b, widgets: [...ws, { ...child, rect: landing }] };
      });
    },
    [change],
  );

  /** Move a widget to the end of the list (= painted on top) when it's interacted with. SILENT: it
   *  fires on every grab, so recording it would make merely touching a card an undo step — and undoing
   *  a z-order change nobody asked for is worse than not being able to. */
  const raiseWidget = useCallback(
    (id: string) => {
      change((b) => {
        if (b.widgets[b.widgets.length - 1]?.id === id) return b;
        const w = b.widgets.find((x) => x.id === id);
        if (!w) return b;
        return { ...b, widgets: [...b.widgets.filter((x) => x.id !== id), w] };
      }, SILENT);
    },
    [change],
  );

  /** The same, one level down: raise a child within its folder's own board, where cards overlap just
   *  like they do outside. Separate from {@link raiseWidget}, which only walks the top level. Silent
   *  for the same reason. */
  const raiseChild = useCallback(
    (folderId: string, childId: string) => {
      change((b) => {
        const folder = b.widgets.find((w) => w.id === folderId && w.kind === "folder");
        const kids = folder?.children;
        if (!kids || kids[kids.length - 1]?.id === childId) return b;
        const child = kids.find((c) => c.id === childId);
        if (!child) return b;
        return {
          ...b,
          widgets: b.widgets.map((w) =>
            w.id === folderId
              ? { ...w, children: [...kids.filter((c) => c.id !== childId), child] }
              : w,
          ),
        };
      }, SILENT);
    },
    [change],
  );

  const addTimelineItem = useCallback(
    (id: string) => {
      const item: TimelineItem = { id: makeId(), date: "", label: "" };
      change((b) => ({
        ...b,
        widgets: mapWidget(b.widgets, id, (w) => ({ ...w, items: [...(w.items ?? []), item] })),
      }));
    },
    [change],
  );

  const updateTimelineItem = useCallback(
    (id: string, itemId: string, patch: Partial<TimelineItem>) => {
      change(
        (b) => ({
          ...b,
          widgets: mapWidget(b.widgets, id, (w) => ({
            ...w,
            items: (w.items ?? []).map((it) => (it.id === itemId ? { ...it, ...patch } : it)),
          })),
        }),
        // Typed into, like a note — grouped per entry so an undo takes a few seconds of the label,
        // not one character.
        { mode: "push", key: `item:${id}:${itemId}` },
      );
    },
    [change],
  );

  const removeTimelineItem = useCallback(
    (id: string, itemId: string) => {
      change((b) => ({
        ...b,
        widgets: mapWidget(b.widgets, id, (w) => ({
          ...w,
          items: (w.items ?? []).filter((it) => it.id !== itemId),
        })),
      }));
    },
    [change],
  );

  return {
    board,
    /** False until the stored board has arrived. The add buttons wait on it: a widget created before
     *  the load lands is overwritten by it, and with a history that would be an unrecoverable loss. */
    ready,
    undo: undoBoard,
    redo: redoBoard,
    canUndo: canUndo(hist),
    canRedo: canRedo(hist),
    addNote,
    addTimeline,
    addFolder,
    updateWidget,
    moveWidget,
    dropWidget,
    removeWidget,
    raiseWidget,
    raiseChild,
    popOutChild,
    addTimelineItem,
    updateTimelineItem,
    removeTimelineItem,
  };
}
