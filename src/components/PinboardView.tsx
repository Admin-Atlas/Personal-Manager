// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import {
  forwardRef,
  memo,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";
import { formatDateOnly } from "../lib/format";
import { runMutation } from "../lib/runMutation";
import {
  addMilestone,
  deleteMilestone,
  ingestNote,
  listDocuments,
  listMilestones,
  listProjects,
  setMilestoneStatus,
  updateMilestone,
} from "../lib/ipc";
import { Markdown } from "../lib/markdown";
import { CELL, folderAtPointer } from "../lib/pinboard/grid";
import {
  applyLineMarker,
  caretForRestore,
  continueList,
  indentLines,
  listIndentBeforeCaret,
  outdentLines,
  toRenderMarkdown,
  toggleTaskAt,
  toggleWrap,
  type TextEdit,
} from "../lib/pinboard/notesMarkdown";
import { rectToPx, useBoardDrag, type DragMode, type PxRect } from "../lib/pinboard/useBoardDrag";
import {
  isPastTimelineDate,
  readConfirmDelete,
  readShowCompletedTimelineItems,
  readShowPastTimelineItems,
  writeConfirmDelete,
  writeShowCompletedTimelineItems,
  writeShowPastTimelineItems,
} from "../lib/pinboard/prefs";
import { todayIso } from "../lib/dateField";
import { usePinboard } from "../lib/pinboard/usePinboard";
// The tint set + names live in one place (src/lib/pinboard/palette.ts) so the board's colours
// stay consistent; the colour VALUES are the global `--st-*` tokens in index.css.
import { NOTE_COLORS, TINT_NAME } from "../lib/pinboard/palette";
import type { CellPoint, Rect, TimelineItem, Widget } from "../lib/pinboard/types";
import type { Milestone, MilestoneStatus } from "../lib/types";
import { scrollBehavior, useDepth } from "../theme";
import { DateField } from "./DateField";
// The status vocabulary + the "what does a pre-status row read as" fallback live with the project
// milestone list, so the two surfaces can never drift into offering different options.
import { MILESTONE_STATUSES, milestoneStatus } from "./MilestoneList";
import { Button, ConfirmDialog, Modal, SegmentedControl, Select, Textarea, Tooltip } from "./ui";

/** The live filing state of a note's ingested document, keyed by `note:<widgetId>`. */
type DocStatus = { reviewed: boolean; project: string };

/** A cheap, stable string hash (djb2) — just to tell whether a note's text has changed since it
 *  was last ingested. Not cryptographic and not the backend content hash. */
function cheapHash(s: string): string {
  let h = 5381;
  for (let i = 0; i < s.length; i++) h = (((h << 5) + h) ^ s.charCodeAt(i)) | 0;
  return (h >>> 0).toString(36);
}

/** Find a widget by id anywhere on the board — top level or inside a folder (folders never nest, so
 *  a depth-1 walk reaches everything). */
function findWidget(widgets: Widget[], id: string): Widget | undefined {
  for (const w of widgets) {
    if (w.id === id) return w;
    const child = w.children?.find((c) => c.id === id);
    if (child) return child;
  }
  return undefined;
}

/** What deleting this card actually costs — the part worth being told before you agree to it. A note
 *  and a timeline are both just gone, but each can have left something behind that the board can't
 *  take back with it. */
function deleteCost(w: Widget, status?: DocStatus): string | null {
  if (w.kind === "note" && w.ingestedAt) {
    const where = status?.reviewed ? `filed under ${status.project}` : "waiting in Review";
    return `You ingested this note, and that document (${where}) is its own copy in your vault — it stays there, and deleting this note won't remove it.`;
  }
  if (w.kind === "timeline" && !w.project && (w.items?.length ?? 0) > 0) {
    const n = w.items?.length ?? 0;
    const dated = w.items?.filter((it) => it.date).length ?? 0;
    return dated > 0
      ? `Its ${n} ${n === 1 ? "entry goes" : "entries go"} with it, and ${dated === 1 ? "the dated one disappears" : `the ${dated} dated ones disappear`} from your calendar.`
      : `Its ${n} ${n === 1 ? "entry goes" : "entries go"} with it.`;
  }
  if (w.kind === "timeline" && w.project) {
    return `This timeline is linked to ${w.project}. Only the card goes — the project's milestones stay exactly as they are.`;
  }
  return null;
}

/** A cell rect as absolute-positioning styles — for overlays drawn over a widget's own tile. */
function rectToPxStyle(r: Rect): CSSProperties {
  const px = rectToPx(r);
  return { left: px.x, top: px.y, width: px.w, height: px.h };
}

/** The tint applied to a widget tile (and its folder-panel card) — a soft wash of its colour token
 *  over the panel surface, or a neutral panel when untinted. */
function tintStyle(color?: string): CSSProperties {
  return color
    ? {
        background: `color-mix(in oklab, var(--${color}) 14%, var(--panel))`,
        borderColor: `color-mix(in oklab, var(--${color}) 35%, var(--border))`,
      }
    : { background: "var(--panel)", borderColor: "var(--border)" };
}

/** The inline (expand-in-place) folder panel's fixed size, in cells → px. */
const PANEL_W = 24 * CELL;
const PANEL_H = 17 * CELL;

/** The share of the main board an opened folder's overlay takes up. */
const OVERLAY_SHARE = 0.8;

/**
 * A board canvas: the ruled grid every widget is positioned against, sized in cells. Shared by the
 * main board and a folder's overlay board so the two can't drift apart — `ref` is what turns a
 * viewport pointer into a board cell, so it must be THIS element (its padding box is the origin
 * absolutely-positioned tiles resolve against).
 */
const BoardSurface = forwardRef<
  HTMLDivElement,
  {
    bounds: { cols: number; rows: number };
    /** Suppress text selection while a gesture is in flight. */
    dragging?: boolean;
    className?: string;
    dataHelp?: string;
    children: ReactNode;
  }
>(function BoardSurface({ bounds, dragging, className, dataHelp, children }, ref) {
  return (
    <div
      ref={ref}
      data-help={dataHelp}
      className={`relative rounded-[var(--radius)] border border-border ${
        dragging ? "select-none" : ""
      } ${className ?? ""}`}
      style={{
        width: bounds.cols * CELL,
        height: bounds.rows * CELL,
        backgroundColor: "var(--surface)",
        backgroundImage:
          "linear-gradient(var(--rule) 1px, transparent 1px), linear-gradient(90deg, var(--rule) 1px, transparent 1px)",
        backgroundSize: `${CELL}px ${CELL}px`,
      }}
    >
      {children}
    </div>
  );
});

/** One widget's tile on a board: positioned at its cell rect, tinted, with the size strip (at power)
 *  and the resize grip. Shared by the main board and a folder's overlay board — a card should look
 *  and behave the same wherever it is, which is the whole point of giving a folder a real board. */
function WidgetTile({
  widget,
  px,
  showPower,
  onStartDrag,
  children,
}: {
  widget: Widget;
  px: PxRect;
  showPower?: boolean;
  onStartDrag: (e: ReactPointerEvent, w: Widget, mode: DragMode) => void;
  children: ReactNode;
}) {
  return (
    <div
      data-help={
        widget.kind === "note"
          ? "pinboard-note"
          : widget.kind === "timeline"
            ? "pinboard-timeline"
            : "pinboard-folder"
      }
      className="absolute flex flex-col overflow-hidden rounded-[var(--radius-sm)] border shadow-sm transition-shadow hover:shadow-md motion-reduce:transition-none"
      style={{ left: px.x, top: px.y, width: px.w, height: px.h, ...tintStyle(widget.color) }}
    >
      {children}

      {/* A note shows its size inline in its own footer (see NoteBody); timeline/folder keep the
          compact coords strip. */}
      {showPower && widget.kind !== "note" && (
        <div className="shrink-0 border-t border-rule px-2 py-0.5 font-mono text-[0.5625rem] text-ink4">
          {widget.rect.x},{widget.rect.y} · {widget.rect.w}×{widget.rect.h}
        </div>
      )}

      {/* Resize handle (bottom-right) — every widget kind is resizable (folders floor at 3×3). */}
      <div
        onPointerDown={(e) => onStartDrag(e, widget, "resize")}
        title="Resize"
        aria-label="Resize widget"
        className="absolute bottom-0 right-0 h-3.5 w-3.5 cursor-nwse-resize touch-none"
        style={{
          background:
            "linear-gradient(135deg, transparent 50%, color-mix(in oklab, var(--ink4) 50%, transparent) 50%)",
        }}
      />
    </div>
  );
}

/**
 * The Pinboard (spec §4): a bounded planning board of draggable, resizable widgets —
 * post-it notes and simple dated timelines — persisted locally. Hand-rolled on pointer
 * events + CSS transforms with grid-snap (no layout library); the snap/clamp maths live in
 * `lib/pinboard/grid.ts`. The board is sized to the window (cell size and fonts stay fixed): a board
 * authored on a wider screen reflows to fit the current width on load, and the canvas grows only
 * downward to hold content that doesn't fit — so it scrolls vertically when full but never overflows
 * the window horizontally. Notes, timelines and
 * folders are available at every
 * depth; per-widget metadata shows at `power`. Notes are Markdown: they render in place and turn
 * back into an editor on click, with a formatting toolbar, keyboard shortcuts, and smart list
 * continuation (`lib/pinboard/notesMarkdown.ts`).
 */
export function PinboardView() {
  const { showMeta, showPower } = useDepth();
  const scrollRef = useRef<HTMLDivElement>(null);
  // The board is sized to the WINDOW, not the whole device screen. It used to be a fixed canvas the
  // size of `screen.avail*`; on a window that isn't maximised (common on macOS/Linux) that overflowed
  // the viewport and showed scrollbars — the board read as "bigger than the screen". Measure the
  // scroller instead (its p-6 gives a 24px inset each side → −48), floored on the CELL grid, and seed
  // from the window so the first load reflow has a sane width before the layout effect measures.
  const [measured, setMeasured] = useState(() => ({
    cols: Math.max(1, Math.floor((window.innerWidth - 48) / CELL)),
    rows: Math.max(1, Math.floor((window.innerHeight - 48) / CELL)),
  }));
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const measure = () =>
      setMeasured({
        cols: Math.max(1, Math.floor((el.clientWidth - 48) / CELL)),
        rows: Math.max(1, Math.floor((el.clientHeight - 48) / CELL)),
      });
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // usePinboard grows the measured window to contain the board's content and hands the result back as
  // `bounds` — the canvas sizes to it, and the drag clamp (below) reads it, so nothing overflows.
  const {
    board,
    bounds: boardBounds,
    load,
    retryLoad,
    saveFailed,
    undo,
    redo,
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
  } = usePinboard(measured);

  // The drag effect reads the bounds through a ref, so it never re-subscribes its pointer listeners to
  // pick up a new value (the board grows/shrinks as the window resizes and content is added or filed).
  const boardBoundsRef = useRef(boardBounds);
  boardBoundsRef.current = boardBounds;

  // The board element itself (scrollRef is its scroller) — needed to turn a viewport pointer
  // position into a board cell, which is what decides whether a drop files into a folder.
  const boardRef = useRef<HTMLDivElement>(null);
  // The folder the pointer is currently over mid-drag (highlighted, and the one a release would file
  // into). Pointer targeting is otherwise invisible — nothing about the dragged rect shows intent.
  const [dropFolderId, setDropFolderId] = useState<string | null>(null);
  // Which folder is currently expanded (transient — not persisted). At most one at a time.
  const [expandedFolderId, setExpandedFolderId] = useState<string | null>(null);
  // A just-added widget id to scroll into view (set by the add buttons; cleared once scrolled).
  const [pendingScrollId, setPendingScrollId] = useState<string | null>(null);

  // The widget list, which the drag reads to find the folder under the pointer, is likewise read
  // through a ref: as a dep it would re-subscribe the pointer listeners on every keystroke (a note's
  // text lives in the board).
  const widgetsRef = useRef(board.widgets);
  widgetsRef.current = board.widgets;

  // Live filing state of any ingested notes, so a note can show "in review" / "filed to X" and
  // reflect a later review made in the Review tab. One list_documents read on mount + on focus.
  const [docStatus, setDocStatus] = useState<Map<string, DocStatus>>(new Map());
  const refreshDocs = useCallback(() => {
    listDocuments()
      .then((docs) => {
        const map = new Map<string, DocStatus>();
        for (const d of docs) {
          if (d.source_id) map.set(d.source_id, { reviewed: d.reviewed, project: d.project });
        }
        setDocStatus(map);
      })
      .catch(() => {
        /* leave the last known statuses */
      });
  }, []);
  useEffect(() => refreshDocs(), [refreshDocs]);
  useEffect(() => {
    const onFocus = () => refreshDocs();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [refreshDocs]);

  // Undo/redo for the whole board. Scope is simply "the Pinboard is open": this view is mounted only
  // while it's the current tab, so a window listener is already scoped to it. (Do NOT gate on focus
  // being inside the board — deleting a card leaves focus on <body>, i.e. "outside", which would kill
  // Ctrl+Z exactly when it's most wanted.)
  //
  // We must OWN the shortcut rather than let the textarea's native undo run: the note's textarea is
  // controlled, so native undo already launders itself into board state through onChange, and the
  // formatting/list helpers write values through React, which corrupts the native stack anyway. Two
  // listeners because keydown alone misses the WebView's own Edit ▸ Undo (context menu): `beforeinput`
  // catches the resulting `historyUndo`. `beforeinput` is scoped to the board element so we never
  // interfere with native undo in the rest of the app.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey) || e.altKey) return;
      // e.code, not e.key: the physical Z/Y, so a non-QWERTY layout still works (matches formatForKey).
      const isZ = e.code === "KeyZ";
      const isY = e.code === "KeyY";
      if (!isZ && !isY) return;
      // Ctrl+Y and Ctrl/Cmd+Shift+Z both redo — Windows and mac conventions respectively, and both
      // are common on Linux.
      const wantRedo = isY || e.shiftKey;
      if (isY && e.shiftKey) return;
      e.preventDefault();
      if (wantRedo) redo();
      else undo();
    };
    const onBeforeInput = (e: Event) => {
      const t = (e as InputEvent).inputType;
      if (t !== "historyUndo" && t !== "historyRedo") return;
      e.preventDefault();
      if (t === "historyRedo") redo();
      else undo();
    };
    window.addEventListener("keydown", onKey);
    const el = boardRef.current;
    el?.addEventListener("beforeinput", onBeforeInput);
    return () => {
      window.removeEventListener("keydown", onKey);
      el?.removeEventListener("beforeinput", onBeforeInput);
    };
  }, [undo, redo]);

  // Track the folder under the pointer so it can be ringed — on THIS board a drop may file into one.
  // (The folder overlay's board passes no equivalent: folders don't nest, so nothing files in there.)
  const trackDropFolder = useCallback(
    (id: string, pointer: CellPoint | null) =>
      setDropFolderId(folderAtPointer(widgetsRef.current, id, pointer)?.id ?? null),
    [],
  );
  // The board's drag/resize gesture — the same hook the folder overlay's board uses.
  const { draggingId, livePx, startDrag } = useBoardDrag({
    boundsRef: boardBoundsRef,
    surfaceRef: boardRef,
    onGrab: raiseWidget,
    onMoveEnd: dropWidget,
    onResizeEnd: moveWidget,
    onPointerCell: trackDropFolder,
  });

  // Add buttons remember the new widget so it can be scrolled into view (a note placed on a lower
  // row would otherwise be created off-screen).
  const handleAddNote = useCallback(() => setPendingScrollId(addNote()), [addNote]);
  const handleAddTimeline = useCallback(() => setPendingScrollId(addTimeline()), [addTimeline]);
  const handleAddFolder = useCallback(() => setPendingScrollId(addFolder()), [addFolder]);
  useLayoutEffect(() => {
    if (!pendingScrollId) return;
    const el = scrollRef.current;
    const w = board.widgets.find((x) => x.id === pendingScrollId);
    if (el && w) {
      const PAD = 24; // matches the scroller's p-6
      const px = rectToPx(w.rect);
      const top = el.scrollTop;
      const left = el.scrollLeft;
      // Only scroll when the new widget isn't already fully in view, to avoid a jarring jump.
      const nextTop =
        px.y - PAD < top || px.y + px.h + PAD > top + el.clientHeight
          ? Math.max(0, px.y - PAD)
          : top;
      const nextLeft =
        px.x - PAD < left || px.x + px.w + PAD > left + el.clientWidth
          ? Math.max(0, px.x - PAD)
          : left;
      if (nextTop !== top || nextLeft !== left) {
        el.scrollTo({ top: nextTop, left: nextLeft, behavior: scrollBehavior() });
      }
    }
    setPendingScrollId(null);
  }, [pendingScrollId, board.widgets]);

  // If the expanded folder goes away (ungrouped while open), close the panel. Emptying a folder no
  // longer removes it, so this no longer fires on the last child leaving — the panel stays open on
  // "This folder is empty.", ready to take something back.
  useEffect(() => {
    if (
      expandedFolderId &&
      !board.widgets.some((w) => w.id === expandedFolderId && w.kind === "folder")
    ) {
      setExpandedFolderId(null);
    }
  }, [expandedFolderId, board.widgets]);

  const expandedFolder = expandedFolderId
    ? board.widgets.find((w) => w.id === expandedFolderId && w.kind === "folder")
    : undefined;

  const dropFolder = dropFolderId
    ? board.widgets.find((w) => w.id === dropFolderId && w.kind === "folder")
    : undefined;

  // Deleting a note or timeline asks first, unless the user has turned that off. ONE interception
  // point for all four call sites (board tiles + both folder views), because it's the same
  // removeWidget underneath. A FOLDER's ✕ is deliberately not routed through here: it ungroups,
  // spilling its cards back onto the board, so there's nothing to lose — and undo covers it.
  const [pendingDelete, setPendingDelete] = useState<Widget | null>(null);
  const [dontAskAgain, setDontAskAgain] = useState(false);
  const requestDelete = useCallback(
    (id: string) => {
      const w = findWidget(board.widgets, id);
      // Read the pref here, not on mount: Settings is an overlay over a still-mounted Pinboard, so a
      // cached copy would go stale the moment the toggle was flipped behind it.
      if (!w || !readConfirmDelete()) {
        removeWidget(id);
        return;
      }
      setDontAskAgain(false);
      setPendingDelete(w);
    },
    [board.widgets, removeWidget],
  );
  const confirmDelete = useCallback(() => {
    if (!pendingDelete) return;
    if (dontAskAgain) writeConfirmDelete(false);
    removeWidget(pendingDelete.id);
    setPendingDelete(null);
  }, [pendingDelete, dontAskAgain, removeWidget]);

  // An opened folder's overlay is 80% of the board (now window-sized), clamped to the scrim's own box
  // — which starts below the h-9 title bar and is inset by its p-6 — so it always fits the window.
  const overlaySize = useMemo(() => {
    const SCRIM_PAD = 24; // the scrim's p-6
    const TITLE_BAR = 36; // the scrim's top-9
    return {
      width: Math.min(boardBounds.cols * CELL * OVERLAY_SHARE, window.innerWidth - SCRIM_PAD * 2),
      height: Math.min(
        boardBounds.rows * CELL * OVERLAY_SHARE,
        window.innerHeight - TITLE_BAR - SCRIM_PAD * 2,
      ),
    };
  }, [boardBounds]);

  return (
    // min-w-0 / min-h-0 keep the oversized board penned inside its own overflow-auto scroller
    // (below) instead of inflating this column and pushing the header's buttons off-screen.
    <div className="flex h-full min-w-0 min-h-0 flex-1 flex-col">
      <header className="flex items-center justify-between gap-3 border-b border-rule px-6 py-3">
        <div className="min-w-0">
          <h1 className="font-head text-lg font-semibold text-ink">Pinboard</h1>
          {/* The save warning is NOT gated on `showMeta`: the prose below it is chrome and folds away
              at min Depth, but "your work isn't being saved" is a loss warning, and those show at
              every Depth. It also replaces the prose rather than sitting under it — the last line of
              that sentence is the very promise being broken. */}
          {saveFailed ? (
            <p className="text-xs text-st-due">
              Couldn&rsquo;t save your board to this device — recent changes may be lost. Your next
              change will try again.
            </p>
          ) : (
            showMeta && (
              <p className="text-xs text-ink4">
                A space to think — drag to arrange, resize from the corner. {MOD}Z undoes.{" "}
                {load === "ready" && "Saved on this device."}
              </p>
            )
          )}
        </div>
        <div className="flex items-center gap-2">
          {showPower && board.widgets.length > 0 && (
            <span className="mr-1 font-mono text-[0.625rem] uppercase tracking-wide text-ink4">
              {board.widgets.length} item{board.widgets.length === 1 ? "" : "s"}
            </span>
          )}
          <Button
            variant="secondary"
            size="sm"
            onClick={handleAddNote}
            disabled={load !== "ready"}
            data-help="pinboard-add-note"
          >
            + Note
          </Button>
          {/* Notes and timelines are both available at every density. */}
          <Button
            variant="secondary"
            size="sm"
            onClick={handleAddTimeline}
            disabled={load !== "ready"}
            data-help="pinboard-add-timeline"
          >
            + Timeline
          </Button>
          {/* An empty folder, made deliberately — the counterpart to stacking two cards to fold them. */}
          <Button
            variant="secondary"
            size="sm"
            onClick={handleAddFolder}
            disabled={load !== "ready"}
            data-help="pinboard-add-folder"
          >
            + Folder
          </Button>
        </div>
      </header>

      <div ref={scrollRef} className="pm-scrollbars min-h-0 min-w-0 flex-1 overflow-auto p-6">
        <BoardSurface
          ref={boardRef}
          bounds={boardBounds}
          dragging={!!draggingId}
          dataHelp="pinboard-board"
        >
          {/* A failed load leaves the SAME empty board on screen as a fresh install, so it must say
              which one this is. "Add a note to start planning" would be two lies at once: that the
              board is empty, and that what you add to it stays. */}
          {load === "failed" ? (
            <div className="absolute inset-0 flex items-center justify-center">
              <div className="max-w-sm px-6 text-center">
                <p className="text-sm text-st-due">Couldn&rsquo;t open your pinboard.</p>
                <p className="mt-1 text-xs text-ink4">
                  Your board is still saved — PM just couldn&rsquo;t read it, so it won&rsquo;t
                  change anything until it can.
                </p>
                <Button variant="secondary" size="sm" onClick={retryLoad} className="mt-3">
                  Retry
                </Button>
              </div>
            </div>
          ) : (
            board.widgets.length === 0 && (
              <div className="pointer-events-none absolute inset-0 flex items-center justify-center">
                <p className="text-sm text-ink4">
                  Add a note to start planning — it stays here between visits.
                </p>
              </div>
            )
          )}

          {board.widgets.map((w) => (
            <WidgetTile
              key={w.id}
              widget={w}
              px={draggingId === w.id && livePx ? livePx : rectToPx(w.rect)}
              showPower={showPower}
              onStartDrag={startDrag}
            >
              {/* The widget owns its own header + body, so note-specific controls (Ingest) can sit
                  in the top bar while their state stays inside the memoised body. */}
              {w.kind === "note" ? (
                <NoteBody
                  widget={w}
                  showPower={showPower}
                  onChange={updateWidget}
                  onDelete={requestDelete}
                  onStartDrag={startDrag}
                  status={docStatus.get(`note:${w.id}`)}
                  onIngested={refreshDocs}
                />
              ) : w.kind === "timeline" ? (
                <TimelineBody
                  widget={w}
                  showPower={showPower}
                  onChange={updateWidget}
                  onDelete={requestDelete}
                  onStartDrag={startDrag}
                  onAddItem={addTimelineItem}
                  onUpdateItem={updateTimelineItem}
                  onRemoveItem={removeTimelineItem}
                />
              ) : (
                <FolderTile
                  widget={w}
                  onChange={updateWidget}
                  onUngroup={removeWidget}
                  onStartDrag={startDrag}
                  onOpen={() => setExpandedFolderId(w.id)}
                />
              )}
            </WidgetTile>
          ))}

          {/* The folder the pointer is over mid-drag: releasing here files the card INTO it, instead
              of leaving it lying on top. Drawn as an overlay rather than a ring on the folder's own
              tile because the dragged widget is raised above it and would hide the cue. */}
          {dropFolder && (
            <div
              aria-hidden="true"
              className="pointer-events-none absolute z-20 rounded-[var(--radius-sm)] ring-2 ring-accent"
              style={rectToPxStyle(dropFolder.rect)}
            />
          )}

          {/* Expanded folder (transient UI, at most one). Rendered as a board sibling so the inline
              panel escapes the tiles' overflow-hidden and paints above them; the tile's rect never moves. */}
          {expandedFolder &&
            ((expandedFolder.expandMode ?? "inline") === "overlay" ? (
              <Modal
                open
                onClose={() => setExpandedFolderId(null)}
                // The one dialog that names itself with `label` rather than `labelledBy`: its title
                // is an editable <input>, not a heading, so there is no element whose text stays
                // the name. Falls back to the placeholder the input shows when it is empty.
                label={expandedFolder.title?.trim() || "Folder"}
                // A share of the board, replacing Modal's own width/height/overflow defaults rather
                // than competing with them. `overlaySize` (above) is 80% of the window-sized board,
                // clamped to the scrim so it always fits.
                widthClassName=""
                heightClassName=""
                overflowClassName="overflow-hidden"
                className="flex flex-col"
                style={overlaySize}
              >
                <FolderBoard
                  folder={expandedFolder}
                  showPower={showPower}
                  onChange={updateWidget}
                  onDelete={requestDelete}
                  onPopOut={popOutChild}
                  onRaiseChild={raiseChild}
                  onAddItem={addTimelineItem}
                  onUpdateItem={updateTimelineItem}
                  onRemoveItem={removeTimelineItem}
                  docStatus={docStatus}
                  onIngested={refreshDocs}
                  onClose={() => setExpandedFolderId(null)}
                />
              </Modal>
            ) : (
              <div
                className="absolute z-30 flex flex-col overflow-hidden rounded-[var(--radius)] border border-border2 bg-panel shadow-2xl"
                style={{
                  left: Math.max(
                    0,
                    Math.min(expandedFolder.rect.x * CELL, boardBounds.cols * CELL - PANEL_W),
                  ),
                  top: Math.max(
                    0,
                    Math.min(expandedFolder.rect.y * CELL, boardBounds.rows * CELL - PANEL_H),
                  ),
                  width: PANEL_W,
                  height: PANEL_H,
                }}
              >
                <FolderPanel
                  folder={expandedFolder}
                  onChange={updateWidget}
                  onDelete={requestDelete}
                  onPopOut={popOutChild}
                  onAddItem={addTimelineItem}
                  onUpdateItem={updateTimelineItem}
                  onRemoveItem={removeTimelineItem}
                  docStatus={docStatus}
                  onIngested={refreshDocs}
                  onClose={() => setExpandedFolderId(null)}
                />
              </div>
            ))}
        </BoardSurface>
      </div>

      {/* Deleting a card is the one thing on the board you can't get back by dragging. Ctrl+Z covers
          it now, but that only helps someone who knows about Ctrl+Z and notices in time. */}
      <ConfirmDialog
        open={!!pendingDelete}
        title={pendingDelete?.kind === "timeline" ? "Delete this timeline?" : "Delete this note?"}
        danger
        confirmLabel="Delete"
        onConfirm={confirmDelete}
        onClose={() => setPendingDelete(null)}
      >
        {pendingDelete && (
          <>
            <p>
              {pendingDelete.title?.trim() ? (
                <>
                  “<span className="text-ink2">{pendingDelete.title.trim()}</span>” will be removed
                  from the board.
                </>
              ) : (
                <>This card will be removed from the board.</>
              )}{" "}
              {deleteCost(pendingDelete, docStatus.get(`note:${pendingDelete.id}`))}
            </p>
            <label className="mt-3 flex items-center gap-2 text-xs text-ink3">
              <input
                type="checkbox"
                checked={dontAskAgain}
                onChange={(e) => setDontAskAgain(e.target.checked)}
                className="h-3.5 w-3.5 accent-[var(--accent)]"
              />
              Don&apos;t ask again (you can turn this back on in Settings)
            </label>
          </>
        )}
      </ConfirmDialog>
    </div>
  );
}

// Platform-aware modifier glyphs so each formatting tooltip names the real shortcut.
const IS_MAC = typeof navigator !== "undefined" && /mac/i.test(navigator.platform || "");
const MOD = IS_MAC ? "⌘" : "Ctrl+";
const SHIFT = IS_MAC ? "⇧" : "Shift+";

/** One formatting-toolbar button: a pictogram, a label, the keyboard shortcut it mirrors (shown in
 *  the tooltip and wired in NoteBody's onKeyDown), and the pure edit it applies. */
interface FormatAction {
  key: string;
  label: string;
  hint: string;
  icon: ReactNode;
  apply: (value: string, selStart: number, selEnd: number) => TextEdit;
}

const FORMAT_ACTIONS: FormatAction[] = [
  {
    key: "bold",
    label: "Bold",
    hint: `${MOD}B`,
    icon: <span className="text-[0.6875rem] font-bold leading-none">B</span>,
    apply: (v, s, e) => toggleWrap(v, s, e, "**"),
  },
  {
    key: "italic",
    label: "Italic",
    hint: `${MOD}I`,
    icon: <span className="font-serif text-[0.6875rem] italic leading-none">I</span>,
    apply: (v, s, e) => toggleWrap(v, s, e, "*"),
  },
  {
    key: "heading",
    label: "Heading",
    hint: `${MOD}${SHIFT}H`,
    icon: <span className="text-[0.6875rem] font-bold leading-none">H</span>,
    apply: (v, s, e) => applyLineMarker(v, s, e, "heading"),
  },
  {
    key: "bullet",
    label: "Bullet list",
    hint: `${MOD}${SHIFT}8`,
    icon: <BulletIcon />,
    apply: (v, s, e) => applyLineMarker(v, s, e, "bullet"),
  },
  {
    key: "number",
    label: "Numbered list",
    hint: `${MOD}${SHIFT}7`,
    icon: <NumberIcon />,
    apply: (v, s, e) => applyLineMarker(v, s, e, "number"),
  },
  {
    key: "checkbox",
    label: "Checklist",
    hint: `${MOD}${SHIFT}9`,
    icon: <CheckboxIcon />,
    apply: (v, s, e) => applyLineMarker(v, s, e, "checkbox"),
  },
];

/** Map a keydown to its formatting action, or null. Kept beside FORMAT_ACTIONS so the shortcuts
 *  and the tooltip hints can't drift apart. */
function formatForKey(e: ReactKeyboardEvent<HTMLTextAreaElement>): FormatAction | null {
  if (!(e.metaKey || e.ctrlKey)) return null;
  if (e.shiftKey) {
    if (e.code === "Digit8") return FORMAT_ACTIONS.find((a) => a.key === "bullet") ?? null;
    if (e.code === "Digit7") return FORMAT_ACTIONS.find((a) => a.key === "number") ?? null;
    if (e.code === "Digit9") return FORMAT_ACTIONS.find((a) => a.key === "checkbox") ?? null;
    if (e.code === "KeyH") return FORMAT_ACTIONS.find((a) => a.key === "heading") ?? null;
    return null;
  }
  if (e.code === "KeyB") return FORMAT_ACTIONS.find((a) => a.key === "bold") ?? null;
  if (e.code === "KeyI") return FORMAT_ACTIONS.find((a) => a.key === "italic") ?? null;
  return null;
}

/** The shared top bar for every widget: an editable title, an optional right-side `actions` slot,
 *  and the delete/✕ control. It is the drag handle when `onStartDrag` is given (board widgets);
 *  folder-panel children pass none, so their card header is a plain, non-draggable bar. */
function WidgetHeader({
  widget,
  placeholder,
  onRename,
  onDelete,
  onStartDrag,
  deleteLabel = "Delete",
  actions,
}: {
  widget: Widget;
  placeholder: string;
  onRename: (title: string) => void;
  onDelete: () => void;
  onStartDrag?: (e: ReactPointerEvent) => void;
  deleteLabel?: string;
  actions?: ReactNode;
}) {
  return (
    <div
      onPointerDown={onStartDrag}
      className={`flex shrink-0 items-center justify-between gap-1 border-b border-rule px-2 py-1 ${
        onStartDrag ? "cursor-grab touch-none active:cursor-grabbing" : ""
      }`}
    >
      {/* stopPropagation on pointerdown so a click edits the title instead of starting a drag
          (mirrors the ✕ button). `basis-12 grow shrink` — NOT `flex-1`, whose `flex-basis: 0` means
          the title only ever gets *leftover* space, and so collapses to nothing on a narrow card
          (a folder-panel one) where there is none. A real basis reserves it a floor first. */}
      <input
        value={widget.title ?? ""}
        onChange={(e) => onRename(e.target.value)}
        onPointerDown={(e) => e.stopPropagation()}
        placeholder={placeholder}
        aria-label={`${placeholder} title`}
        className="min-w-0 shrink grow basis-12 truncate border-0 bg-transparent px-0 text-xs font-medium text-ink3 placeholder:text-ink4 focus:text-ink2 focus:outline-none focus:ring-0"
      />
      {/* A drag grip so a long title still leaves somewhere to grab the header — only where the
          header IS a handle. Folder-panel cards aren't draggable, so the spacer was 24px of a narrow
          card's budget spent on nothing. It used to be blank space: the whole bar drags, but with
          nothing drawn there you had to hunt for a spot that wasn't the title or a button. The dots
          just SHOW where the reliable grab point is — the handler still lives on the bar, so this
          needs no pointer logic of its own. */}
      {onStartDrag && <DragGrip />}
      {/* min-w-0 shrink, not shrink-0: the actions must be able to give, or they overflow the card
          (which is overflow-hidden) and the ✕ on the end is what gets clipped away. */}
      <div className="flex min-w-0 shrink items-center gap-1">
        {actions}
        <button
          onPointerDown={(e) => e.stopPropagation()}
          onClick={onDelete}
          aria-label={deleteLabel}
          title={deleteLabel}
          className="inline-flex min-h-[var(--tap-min,24px)] min-w-[var(--tap-min,24px)] shrink-0 items-center justify-center rounded-[var(--radius-sm)] text-xs text-ink4 hover:bg-surface hover:text-st-due"
        >
          ✕
        </button>
      </div>
    </div>
  );
}

/** One of a timeline card's filter checkboxes — "Past" (by date) or "Done" (by status). One
 *  component rather than two near-identical ones, so the two read and behave identically; the
 *  caller owns the pref write, since each answers a different question and they persist separately.
 *
 *  `onChange` is expected to write through as well as set state, so every timeline card on the board
 *  agrees and the choice survives a remount (a tab switch unmounts the whole board). */
function TimelineFilterToggle({
  label,
  title,
  checked,
  onChange,
}: {
  label: string;
  title: string;
  checked: boolean;
  onChange: (v: boolean) => void;
}) {
  return (
    <label
      className="flex shrink-0 items-center gap-1 text-[0.625rem] uppercase tracking-wide text-ink4"
      title={title}
      onPointerDown={(e) => e.stopPropagation()}
    >
      <input
        type="checkbox"
        checked={checked}
        onChange={(e) => onChange(e.currentTarget.checked)}
        className="accent-[var(--accent)]"
      />
      {label}
    </label>
  );
}

/** The 2×3 dot grip drawn in a draggable widget header, immediately left of the header's actions —
 *  so on a note it sits beside the ingest control, and on a folder-overlay child beside its pop-out.
 *  Purely a visual affordance: `aria-hidden` and `pointer-events-none` so it neither appears to
 *  assistive tech as a control nor intercepts the pointerdown the header itself is listening for. */
function DragGrip() {
  return (
    <svg
      viewBox="0 0 6 10"
      aria-hidden="true"
      className="pointer-events-none w-1.5 shrink-0 self-center text-ink4"
      fill="currentColor"
    >
      {[1, 5].map((cx) =>
        [1, 5, 9].map((cy) => <circle key={`${cx}-${cy}`} cx={cx} cy={cy} r="1" />),
      )}
    </svg>
  );
}

/** "Move this card out of the folder, back onto the board" — shown on folder-panel children. */
function PopOutButton({ onClick }: { onClick: () => void }) {
  return (
    <button
      type="button"
      onPointerDown={(e) => e.stopPropagation()}
      onClick={onClick}
      title="Move out to the board"
      aria-label="Move out to the board"
      className="inline-flex min-h-[var(--tap-min,24px)] min-w-[var(--tap-min,24px)] shrink-0 items-center justify-center rounded-[var(--radius-sm)] text-xs text-ink4 hover:bg-surface hover:text-ink2"
    >
      ⤴
    </button>
  );
}

/** Which rendered task checkbox (if any) a click at these viewport coordinates landed on.
 *
 *  Hit-tested against each box's own rect rather than by walking up to the `<li>`, because the whole
 *  list item is also the click target for "open the editor" — only the box itself should tick. The
 *  boxes are in document order, which is the order `toggleTaskAt` counts markers in, so the array
 *  index IS the marker index. `PAD` gives the ~13px native box a forgiving target without letting it
 *  reach the text. */
function taskIndexAt(container: HTMLElement, clientX: number, clientY: number): number | null {
  const PAD = 4;
  const boxes = container.querySelectorAll<HTMLInputElement>('li input[type="checkbox"]');
  for (let i = 0; i < boxes.length; i++) {
    const r = boxes[i].getBoundingClientRect();
    if (
      clientX >= r.left - PAD &&
      clientX <= r.right + PAD &&
      clientY >= r.top - PAD &&
      clientY <= r.bottom + PAD
    ) {
      return i;
    }
  }
  return null;
}

// Memoised: a drag/resize sets state on every pointermove, re-rendering the whole board — without
// memo each note would re-run the react-markdown pipeline per tick. All props are stable across a
// board drag: widget objects are only replaced when edited; `onChange`/`onDelete`/`onIngested` and
// the memoised `onStartDrag` are stable useCallbacks; `onPopOut` is absent for board tiles; `status`
// comes out of a map rebuilt only on refetch.
const NoteBody = memo(function NoteBody({
  widget,
  showPower,
  onChange,
  onDelete,
  onStartDrag,
  onPopOut,
  status,
  onIngested,
}: {
  widget: Widget;
  /** At `power` depth, show the note's size (w×h) at the right of its footer. */
  showPower?: boolean;
  onChange: (id: string, patch: Partial<Widget>) => void;
  onDelete: (id: string) => void;
  /** The board drag handle. Absent for folder-panel children (they aren't board-positioned). */
  onStartDrag?: (e: ReactPointerEvent, w: Widget, mode: "move" | "resize") => void;
  /** When set (a folder's child), a "move out to the board" control shows in the header. Takes the
   *  child's id so the parent can pass ONE stable callback rather than a fresh arrow per render,
   *  which would defeat this component's memo and re-run react-markdown on every drag tick. */
  onPopOut?: (id: string) => void;
  status?: DocStatus;
  onIngested: () => void;
}) {
  const text = widget.text ?? "";
  const taRef = useRef<HTMLTextAreaElement>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  // Render-on-idle: a filled note shows rendered Markdown (so lists read as lists); click it — or an
  // empty note — to drop into the textarea, and it re-renders on blur. No manual preview/edit toggle.
  const [editing, setEditing] = useState(false);
  const showEditor = editing || !text.trim();

  // Focus the textarea only when the user actively opens a rendered note for editing — not on load,
  // so existing empty notes don't fight over focus.
  useLayoutEffect(() => {
    if (editing) taRef.current?.focus();
  }, [editing]);

  // An undo/redo swaps `text` out from under the caret, and React parks a controlled textarea's caret
  // at the end when the value shrinks — so undoing a few seconds of typing mid-note would dump you at
  // the bottom of it. Put the caret back where the change actually was.
  //
  // ONLY for a change this note didn't make itself. `selfEdit` is the discriminator, and it has to
  // be: the browser has already placed the caret correctly for a keystroke, and where a string is
  // ambiguous (type "a" at the start of "aaaaa") the text alone cannot tell us where the edit was —
  // guessing would move the caret out from under someone who is simply typing.
  const lastText = useRef(text);
  const selfEdit = useRef(false);
  useLayoutEffect(() => {
    const ta = taRef.current;
    const from = lastText.current;
    lastText.current = text;
    if (selfEdit.current) {
      selfEdit.current = false;
      return;
    }
    if (!ta || from === text || document.activeElement !== ta) return;
    ta.selectionStart = ta.selectionEnd = caretForRestore(from, text);
  }, [text]);
  /** Change this note's text, flagging it as ours so the caret restore above stands aside. */
  const editText = useCallback(
    (next: string) => {
      selfEdit.current = true;
      onChange(widget.id, { text: next });
    },
    [onChange, widget.id],
  );

  // Stay in edit mode until the user clicks elsewhere on the board — NOT when the window merely
  // loses OS focus (tabbing out of the app used to collapse the note, because the textarea blurred).
  // Exit only on a pointer-down outside this note; re-focus the textarea when the window returns
  // while we're still editing, so the caret picks back up.
  useEffect(() => {
    if (!editing) return;
    const onDown = (e: PointerEvent) => {
      if (rootRef.current && !rootRef.current.contains(e.target as Node)) setEditing(false);
    };
    const onWinFocus = () => taRef.current?.focus();
    document.addEventListener("pointerdown", onDown, true);
    window.addEventListener("focus", onWinFocus);
    return () => {
      document.removeEventListener("pointerdown", onDown, true);
      window.removeEventListener("focus", onWinFocus);
    };
  }, [editing]);

  const [ingesting, setIngesting] = useState(false);
  const [ingestErr, setIngestErr] = useState<string | null>(null);
  const ingested = !!widget.ingestedAt;
  // The note has diverged from what was last ingested → offer a re-ingest.
  const edited = ingested && widget.ingestedHash !== cheapHash(text);

  async function ingest() {
    if (!text.trim() || ingesting) return;
    setIngesting(true);
    setIngestErr(null);
    try {
      // Ingest the rendered markdown, not the pinboard shorthand dialect: the vault copy is read
      // everywhere outside the board (reader, retrieval, chat citations), where the raw dialect
      // markers render degraded and get indexed as noise. The widget keeps the raw `text`, and the
      // edit-detection hash still tracks it, so "edited since last ingest" is unaffected.
      await ingestNote(widget.id, widget.title ?? "", toRenderMarkdown(text));
      onChange(widget.id, { ingestedAt: new Date().toISOString(), ingestedHash: cheapHash(text) });
      onIngested();
    } catch (e) {
      setIngestErr(String(e));
    } finally {
      setIngesting(false);
    }
  }

  // Apply a pure text edit (toolbar button or shortcut) through the controlled value, then restore
  // the selection after React re-renders — mirrors the caret-restore already used for list continuation.
  const applyEdit = useCallback(
    (make: (value: string, selStart: number, selEnd: number) => TextEdit) => {
      const ta = taRef.current;
      if (!ta) return;
      const res = make(text, ta.selectionStart, ta.selectionEnd);
      editText(res.text);
      setEditing(true);
      requestAnimationFrame(() => {
        const t = taRef.current;
        if (!t) return;
        t.focus();
        t.selectionStart = res.selStart;
        t.selectionEnd = res.selEnd;
      });
    },
    [text, editText],
  );

  // Cmd/Ctrl formatting shortcuts win first; then Enter continues the current list (next bullet /
  // number / roman / checkbox) or exits it on an empty item; Shift+Enter stays a plain newline.
  function onKeyDown(e: ReactKeyboardEvent<HTMLTextAreaElement>) {
    const fmt = formatForKey(e);
    if (fmt) {
      e.preventDefault();
      applyEdit(fmt.apply);
      return;
    }
    // Tab / Shift+Tab indent / outdent the current line(s). Indenting a checkbox nests it under the
    // item above; continueList carries that indent to the items typed after it.
    if (e.key === "Tab") {
      e.preventDefault();
      applyEdit(e.shiftKey ? outdentLines : indentLines);
      return;
    }
    // Backspace while the caret sits inside a list item's leading indent → outdent, rather than
    // nibbling one space at a time.
    if (
      e.key === "Backspace" &&
      !e.shiftKey &&
      !e.metaKey &&
      !e.ctrlKey &&
      !e.altKey &&
      e.currentTarget.selectionStart === e.currentTarget.selectionEnd &&
      listIndentBeforeCaret(e.currentTarget.value, e.currentTarget.selectionStart) != null
    ) {
      e.preventDefault();
      applyEdit(outdentLines);
      return;
    }
    if (e.key !== "Enter" || e.shiftKey) return;
    const ta = e.currentTarget;
    if (ta.selectionStart !== ta.selectionEnd) return;
    const res = continueList(ta.value, ta.selectionStart);
    if (!res) return;
    e.preventDefault();
    editText(res.text);
    requestAnimationFrame(() => {
      if (taRef.current) taRef.current.selectionStart = taRef.current.selectionEnd = res.caret;
    });
  }

  // The Ingest / filing-status control sits in the top bar (right of the title, left of ✕).
  const ingestControl = !ingested ? (
    <button
      type="button"
      onPointerDown={(e) => e.stopPropagation()}
      onClick={ingest}
      disabled={ingesting || !text.trim()}
      className="shrink-0 rounded-[var(--radius-sm)] px-1 text-[0.625rem] uppercase tracking-wide text-accent-text hover:bg-surface disabled:opacity-40"
      title="Save this note to your vault as a document (it goes through Review)"
    >
      {ingesting ? "Saving…" : "Ingest"}
    </button>
  ) : (
    <>
      <span
        // Shrinkable and capped short: this span is the widest thing in the bar on an ingested note
        // ("Filed · <project>"), and on a folder-panel card it would otherwise push the ✕ out. The
        // full text is on the tooltip, so truncating hard costs nothing.
        className="min-w-0 shrink truncate text-[0.625rem] text-ink4"
        title={
          status
            ? status.reviewed
              ? `Filed under ${status.project}`
              : "Waiting in the Review queue"
            : "Saved to your vault as a document"
        }
      >
        {status ? (status.reviewed ? `Filed · ${status.project}` : "In review") : "Ingested"}
      </span>
      {edited && (
        <button
          type="button"
          onPointerDown={(e) => e.stopPropagation()}
          onClick={ingest}
          disabled={ingesting}
          className="shrink-0 rounded-[var(--radius-sm)] px-1 text-[0.625rem] uppercase tracking-wide text-accent-text hover:bg-surface disabled:opacity-40"
          title="Update the saved document with your latest edits"
        >
          {ingesting ? "…" : "Re-ingest"}
        </button>
      )}
    </>
  );

  return (
    <div ref={rootRef} className="flex min-h-0 flex-1 flex-col" data-help="pinboard-note-ingest">
      <WidgetHeader
        widget={widget}
        placeholder="Note"
        onRename={(t) => onChange(widget.id, { title: t })}
        onDelete={() => onDelete(widget.id)}
        onStartDrag={onStartDrag ? (e) => onStartDrag(e, widget, "move") : undefined}
        actions={
          <>
            {onPopOut && <PopOutButton onClick={() => onPopOut(widget.id)} />}
            {ingestControl}
          </>
        }
      />
      {ingestErr && <p className="shrink-0 px-2 pt-1 text-[0.625rem] text-st-due">{ingestErr}</p>}
      {showEditor ? (
        <Textarea
          ref={taRef}
          value={text}
          onChange={(e) => editText(e.target.value)}
          onKeyDown={onKeyDown}
          onFocus={() => setEditing(true)}
          // No onBlur exit — edit mode ends only on a click outside the note (see the effect above),
          // so tabbing out of the app or clicking the note's own controls keeps it editable.
          placeholder="Jot something down…"
          className="min-h-0 flex-1 resize-none border-0 bg-transparent text-sm leading-snug focus:ring-0"
        />
      ) : (
        <div
          // pm-note-md scopes the flush-checkbox rule to notes (index.css) without touching chat/reader.
          className="pm-note-md min-h-0 flex-1 cursor-text overflow-auto px-2 text-sm"
          // A click on a rendered checkbox ticks it; a click anywhere else opens the editor. The
          // boxes carry `pointer-events: none` (index.css) so the click lands here rather than being
          // swallowed by the disabled input, and the hit test is by COORDINATES against each box's
          // rect — not "did you hit the <li>" — so clicking a task's TEXT still opens the editor.
          onClick={(e) => {
            const idx = taskIndexAt(e.currentTarget, e.clientX, e.clientY);
            if (idx !== null) {
              const next = toggleTaskAt(text, idx);
              if (next !== null) {
                editText(next);
                return;
              }
            }
            setEditing(true);
          }}
          // Keyboard path into edit mode. No role/aria-label — it's the note's content region, so its
          // text stays the accessible name; Enter only edits when the region itself (not a link inside) is focused.
          tabIndex={0}
          onKeyDown={(e) => {
            if (e.target === e.currentTarget && e.key === "Enter") {
              e.preventDefault();
              setEditing(true);
            }
          }}
          title="Click to edit"
        >
          <Markdown>{toRenderMarkdown(text)}</Markdown>
        </div>
      )}
      {/* Footer: only render when it has something to show — the format toolbar + colour swatches
          appear while editing, and the size (w×h) at power depth. An idle filled note has neither,
          so the footer vanishes and the note gains that space. */}
      {(showEditor || showPower) && (
        <div className="flex shrink-0 flex-wrap items-center gap-x-2 gap-y-1 px-2 pb-1">
          {showEditor && (
            <div className="flex items-center gap-0.5" data-help="pinboard-note-format">
              {FORMAT_ACTIONS.map((a) => (
                <Tooltip key={a.key} label={`${a.label} · ${a.hint}`}>
                  <button
                    type="button"
                    aria-label={`${a.label} (${a.hint})`}
                    // Keep the textarea focused/selected so the edit (and its shortcut) lands where
                    // the caret is.
                    onMouseDown={(e) => e.preventDefault()}
                    onClick={() => applyEdit(a.apply)}
                    className="flex h-5 w-5 min-h-[var(--tap-min,24px)] min-w-[var(--tap-min,24px)] items-center justify-center rounded-[var(--radius-sm)] text-ink4 hover:bg-surface hover:text-ink2"
                  >
                    {a.icon}
                  </button>
                </Tooltip>
              ))}
            </div>
          )}
          {/* Right-aligned: colour swatches (edit only) then the size (power). */}
          <div className="ml-auto flex items-center gap-2">
            {showEditor && (
              <div className="flex items-center gap-1" data-help="pinboard-note-tint">
                {NOTE_COLORS.map((c) => {
                  const name = TINT_NAME[c] ?? c.replace("st-", "");
                  return (
                    <Tooltip key={c} label={name}>
                      <button
                        // Don't blur the textarea's caret when picking a colour (mirrors the format
                        // buttons) so edit mode and the selection survive.
                        onMouseDown={(e) => e.preventDefault()}
                        onClick={() => onChange(widget.id, { color: c })}
                        aria-label={`Colour: ${name}`}
                        className={`h-3 w-3 rounded-full border ${
                          widget.color === c ? "ring-1 ring-ink3" : ""
                        }`}
                        style={{
                          background: `var(--${c})`,
                          borderColor: "color-mix(in oklab, var(--ink) 20%, transparent)",
                        }}
                      />
                    </Tooltip>
                  );
                })}
              </div>
            )}
            {showPower && (
              <span className="font-mono text-[0.5625rem] text-ink4">
                {widget.rect.w}×{widget.rect.h}
              </span>
            )}
          </div>
        </div>
      )}
    </div>
  );
});

function BulletIcon() {
  return (
    <svg viewBox="0 0 24 24" className="h-4 w-4" fill="none" stroke="currentColor" strokeWidth={2}>
      <circle cx="4" cy="6" r="1.4" fill="currentColor" stroke="none" />
      <circle cx="4" cy="12" r="1.4" fill="currentColor" stroke="none" />
      <circle cx="4" cy="18" r="1.4" fill="currentColor" stroke="none" />
      <line x1="9" y1="6" x2="20" y2="6" strokeLinecap="round" />
      <line x1="9" y1="12" x2="20" y2="12" strokeLinecap="round" />
      <line x1="9" y1="18" x2="20" y2="18" strokeLinecap="round" />
    </svg>
  );
}

function NumberIcon() {
  return (
    <svg viewBox="0 0 24 24" className="h-4 w-4" fill="none" stroke="currentColor" strokeWidth={2}>
      <line x1="10" y1="6" x2="20" y2="6" strokeLinecap="round" />
      <line x1="10" y1="12" x2="20" y2="12" strokeLinecap="round" />
      <line x1="10" y1="18" x2="20" y2="18" strokeLinecap="round" />
      <text x="1" y="8.5" fontSize="7" fill="currentColor" stroke="none">
        1
      </text>
      <text x="1" y="14.5" fontSize="7" fill="currentColor" stroke="none">
        2
      </text>
      <text x="1" y="20.5" fontSize="7" fill="currentColor" stroke="none">
        3
      </text>
    </svg>
  );
}

function CheckboxIcon() {
  return (
    <svg
      viewBox="0 0 24 24"
      className="h-4 w-4"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      <rect x="4" y="4" width="16" height="16" rx="3" />
      <path d="M8 12l3 3 5-6" />
    </svg>
  );
}

interface TimelineBodyProps {
  widget: Widget;
  showPower: boolean;
  onChange: (id: string, patch: Partial<Widget>) => void;
  onDelete: (id: string) => void;
  /** The board drag handle. Absent in the in-place folder panel, which lays cards out itself. */
  onStartDrag?: (e: ReactPointerEvent, w: Widget, mode: "move" | "resize") => void;
  /** When set (a folder's child), a "move out to the board" control shows in the header. Takes the
   *  child's id so the parent can pass one stable callback (see NoteBody). */
  onPopOut?: (id: string) => void;
  onAddItem: (id: string) => void;
  onUpdateItem: (id: string, itemId: string, patch: { date?: string; label?: string }) => void;
  onRemoveItem: (id: string, itemId: string) => void;
}

/** A timeline is either *bound* to a real project — showing and editing that project's live
 *  milestones, which flow to the brief + Focus — or a freeform scratch list (the default). The
 *  shared header (editable title, drag, delete) wraps either body. Memoised for the same board-wide
 *  drag re-renders as NoteBody (all board-tile props are stable). */
const TimelineBody = memo(function TimelineBody(props: TimelineBodyProps) {
  const { widget, onChange, onDelete, onStartDrag, onPopOut, showPower } = props;
  // Effective layout: an explicit `widget.view`, else each kind's historical default (freeform →
  // list, bound → row) so pre-existing boards look identical until the user toggles.
  const view: TimelineView = widget.view ?? (widget.project ? "row" : "list");
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <WidgetHeader
        widget={widget}
        placeholder="Timeline"
        onRename={(t) => onChange(widget.id, { title: t })}
        onDelete={() => onDelete(widget.id)}
        onStartDrag={onStartDrag ? (e) => onStartDrag(e, widget, "move") : undefined}
        actions={
          <>
            <TimelineViewToggle value={view} onChange={(v) => onChange(widget.id, { view: v })} />
            {onPopOut && <PopOutButton onClick={() => onPopOut(widget.id)} />}
          </>
        }
      />
      <div className="min-h-0 flex-1 overflow-auto">
        {widget.project ? (
          <BoundTimeline
            project={widget.project}
            view={view}
            showPower={showPower}
            // Unbind, and drop any freeform entries the widget still carries. Linking consumes them
            // now, so post-fix this is a no-op — but a board linked by an older build kept its copy,
            // and without this those stale entries would spring back onto the calendar's pinboard
            // overlay beside the milestones they already became. The milestones stay in the project
            // either way, which is exactly what the button promises.
            onUnlink={() => onChange(widget.id, { project: undefined, items: [] })}
          />
        ) : (
          <FreeformTimeline {...props} view={view} />
        )}
      </div>
    </div>
  );
});

type TimelineView = "list" | "row";

/** A compact list ⇄ row toggle for a timeline widget, sitting in its header. stopPropagation so a
 *  click switches views instead of starting a board drag (the header is the drag handle). */
function TimelineViewToggle({
  value,
  onChange,
}: {
  value: TimelineView;
  onChange: (v: TimelineView) => void;
}) {
  const opt = (v: TimelineView, label: string, icon: ReactNode) => (
    <button
      type="button"
      onPointerDown={(e) => e.stopPropagation()}
      onClick={() => onChange(v)}
      aria-pressed={value === v}
      aria-label={label}
      title={label}
      className={`flex h-5 w-5 min-h-[var(--tap-min,24px)] min-w-[var(--tap-min,24px)] items-center justify-center rounded-[var(--radius-sm)] ${
        value === v ? "bg-accent text-accent-ink" : "text-ink4 hover:bg-surface hover:text-ink2"
      }`}
    >
      {icon}
    </button>
  );
  return (
    <div className="flex items-center gap-0.5" data-help="pinboard-timeline-view">
      {opt("list", "List view", <ListViewIcon />)}
      {opt("row", "Timeline view", <RowViewIcon />)}
    </div>
  );
}

function ListViewIcon() {
  return (
    <svg
      viewBox="0 0 24 24"
      className="h-3.5 w-3.5"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
    >
      <line x1="5" y1="7" x2="19" y2="7" />
      <line x1="5" y1="12" x2="19" y2="12" />
      <line x1="5" y1="17" x2="19" y2="17" />
    </svg>
  );
}

function RowViewIcon() {
  return (
    <svg
      viewBox="0 0 24 24"
      className="h-3.5 w-3.5"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
    >
      <line x1="3" y1="12" x2="21" y2="12" strokeLinecap="round" />
      <circle cx="7" cy="12" r="1.8" fill="currentColor" stroke="none" />
      <circle cx="13" cy="12" r="1.8" fill="currentColor" stroke="none" />
      <circle cx="19" cy="12" r="1.8" fill="currentColor" stroke="none" />
    </svg>
  );
}

/** The horizontal-track chrome shared by both row-view timelines: a baseline line under
 *  date-ordered columns that scroll horizontally. */
function TimelineTrack({ children }: { children: ReactNode }) {
  return (
    <div className="relative min-h-0 flex-1 overflow-x-auto" data-help="pinboard-timeline-line">
      {/* the line the dots sit on */}
      <div className="pointer-events-none absolute inset-x-2 top-8 h-px bg-border2" />
      <div className="flex items-start gap-1 pb-1">{children}</div>
    </div>
  );
}

/** The stacked-rows chrome shared by both list-view timelines. */
function TimelineList({ children }: { children: ReactNode }) {
  return <div className="min-h-0 flex-1 space-y-1 overflow-auto">{children}</div>;
}

/** A collapsed folder tile (3×3 by default, resizable like every kind): the shared header (editable
 *  title + Ungroup) over a big button showing the child count, which opens the folder. */
function FolderTile({
  widget,
  onChange,
  onUngroup,
  onStartDrag,
  onOpen,
}: {
  widget: Widget;
  onChange: (id: string, patch: Partial<Widget>) => void;
  onUngroup: (id: string) => void;
  onStartDrag: (e: ReactPointerEvent, w: Widget, mode: "move" | "resize") => void;
  onOpen: () => void;
}) {
  const n = widget.children?.length ?? 0;
  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <WidgetHeader
        widget={widget}
        placeholder="Folder"
        deleteLabel="Ungroup (spill notes back onto the board)"
        onRename={(t) => onChange(widget.id, { title: t })}
        onDelete={() => onUngroup(widget.id)}
        onStartDrag={(e) => onStartDrag(e, widget, "move")}
      />
      <button
        type="button"
        onPointerDown={(e) => e.stopPropagation()}
        onClick={onOpen}
        title="Open folder"
        className="flex min-h-0 flex-1 flex-col items-center justify-center gap-0.5 text-ink3 hover:bg-surface"
      >
        <FolderGlyph />
        <span className="font-mono text-[0.625rem]">
          {n} item{n === 1 ? "" : "s"}
        </span>
      </button>
    </div>
  );
}

function FolderGlyph() {
  return (
    <svg
      viewBox="0 0 24 24"
      className="h-6 w-6"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.6}
    >
      <path
        d="M3 7a1 1 0 0 1 1-1h5l2 2h8a1 1 0 0 1 1 1v8a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1V7Z"
        strokeLinejoin="round"
      />
    </svg>
  );
}

/** The bar both expanded-folder presentations share: editable title, the In place / Overlay choice,
 *  and close. */
function FolderPanelHeader({
  folder,
  onChange,
  onClose,
}: {
  folder: Widget;
  onChange: (id: string, patch: Partial<Widget>) => void;
  onClose: () => void;
}) {
  return (
    <div className="flex shrink-0 items-center justify-between gap-2 border-b border-rule px-3 py-2">
      <input
        value={folder.title ?? ""}
        onChange={(e) => onChange(folder.id, { title: e.target.value })}
        placeholder="Folder"
        aria-label="Folder title"
        className="min-w-0 flex-1 truncate border-0 bg-transparent px-0 text-sm font-medium text-ink2 placeholder:text-ink4 focus:outline-none focus:ring-0"
      />
      <SegmentedControl
        ariaLabel="How this folder opens"
        value={folder.expandMode ?? "inline"}
        onChange={(m) => onChange(folder.id, { expandMode: m })}
        options={[
          { value: "inline", label: "In place" },
          { value: "overlay", label: "Overlay" },
        ]}
      />
      <button
        type="button"
        onClick={onClose}
        title="Close folder"
        aria-label="Close folder"
        className="inline-flex min-h-[var(--tap-min,24px)] min-w-[var(--tap-min,24px)] shrink-0 items-center justify-center rounded-[var(--radius-sm)] text-sm text-ink4 hover:bg-surface hover:text-ink2"
      >
        ✕
      </button>
    </div>
  );
}

/**
 * The OVERLAY presentation: the folder's own pinboard.
 *
 * Cards keep the shape and size they had outside, and are dragged and resized in here with the same
 * gesture, the same grid and the same tile as the main board — a folder is a place to put things,
 * not a different kind of thing. Folders never nest, so a drop in here can't fold or file anything:
 * it is always a plain move, and cards simply overlap. That's why there's no resolveDrop, no pointer
 * cell and no drop highlight — none of them would have anything to decide.
 */
function FolderBoard({
  folder,
  showPower,
  onChange,
  onDelete,
  onPopOut,
  onRaiseChild,
  onAddItem,
  onUpdateItem,
  onRemoveItem,
  docStatus,
  onIngested,
  onClose,
}: {
  folder: Widget;
  showPower: boolean;
  onChange: (id: string, patch: Partial<Widget>) => void;
  onDelete: (id: string) => void;
  onPopOut: (folderId: string, childId: string) => void;
  onRaiseChild: (folderId: string, childId: string) => void;
  onAddItem: (id: string) => void;
  onUpdateItem: (id: string, itemId: string, patch: { date?: string; label?: string }) => void;
  onRemoveItem: (id: string, itemId: string) => void;
  docStatus: Map<string, DocStatus>;
  onIngested: () => void;
  onClose: () => void;
}) {
  const children = useMemo(() => folder.children ?? [], [folder.children]);
  const scrollRef = useRef<HTMLDivElement>(null);
  const surfaceRef = useRef<HTMLDivElement>(null);

  // Measure the body so the canvas fills it. p-3 on the scroller, hence the 12px inset.
  const [measured, setMeasured] = useState({ cols: 1, rows: 1 });
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const PAD = 12;
    const measure = () =>
      setMeasured({
        cols: Math.max(1, Math.floor((el.clientWidth - PAD * 2) / CELL)),
        rows: Math.max(1, Math.floor((el.clientHeight - PAD * 2) / CELL)),
      });
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // The canvas fills the body, then grows to contain any card sitting past it — cards are never
  // re-flowed to fit (that would resize them, which is exactly what this view exists to preserve),
  // so the board scrolls to reach them instead.
  const bounds = useMemo(() => {
    let cols = measured.cols;
    let rows = measured.rows;
    for (const c of children) {
      cols = Math.max(cols, c.rect.x + c.rect.w);
      rows = Math.max(rows, c.rect.y + c.rect.h);
    }
    return { cols, rows };
  }, [measured, children]);
  const boundsRef = useRef(bounds);
  boundsRef.current = bounds;

  // Stable, so the memoised card bodies survive the per-tick re-renders of a drag.
  const popOut = useCallback(
    (childId: string) => onPopOut(folder.id, childId),
    [onPopOut, folder.id],
  );
  const commitRect = useCallback((id: string, rect: Rect) => onChange(id, { rect }), [onChange]);
  const { draggingId, livePx, startDrag } = useBoardDrag({
    boundsRef,
    surfaceRef,
    onGrab: useCallback((id: string) => onRaiseChild(folder.id, id), [onRaiseChild, folder.id]),
    onMoveEnd: commitRect,
    onResizeEnd: commitRect,
  });

  return (
    <div className="flex h-full min-h-0 flex-col">
      <FolderPanelHeader folder={folder} onChange={onChange} onClose={onClose} />
      <div ref={scrollRef} className="pm-scrollbars min-h-0 min-w-0 flex-1 overflow-auto p-3">
        <BoardSurface
          ref={surfaceRef}
          bounds={bounds}
          dragging={!!draggingId}
          dataHelp="pinboard-folder-board"
        >
          {children.length === 0 && (
            <div className="pointer-events-none absolute inset-0 flex items-center justify-center">
              <p className="text-sm text-ink4">
                This folder is empty — drag a note or timeline onto it to file it here.
              </p>
            </div>
          )}
          {children.map((c) => (
            <WidgetTile
              key={c.id}
              widget={c}
              px={draggingId === c.id && livePx ? livePx : rectToPx(c.rect)}
              showPower={showPower}
              onStartDrag={startDrag}
            >
              {c.kind === "note" ? (
                <NoteBody
                  widget={c}
                  showPower={showPower}
                  onChange={onChange}
                  onDelete={onDelete}
                  onStartDrag={startDrag}
                  onPopOut={popOut}
                  status={docStatus.get(`note:${c.id}`)}
                  onIngested={onIngested}
                />
              ) : (
                <TimelineBody
                  widget={c}
                  showPower={showPower}
                  onChange={onChange}
                  onDelete={onDelete}
                  onStartDrag={startDrag}
                  onPopOut={popOut}
                  onAddItem={onAddItem}
                  onUpdateItem={onUpdateItem}
                  onRemoveItem={onRemoveItem}
                />
              )}
            </WidgetTile>
          ))}
        </BoardSurface>
      </div>
    </div>
  );
}

/** The expanded folder view IN PLACE: editable title, a presentation toggle, and a grid of the
 *  contained cards — each the SAME NoteBody/TimelineBody, so children edit/ingest just like board
 *  widgets. Children carry no drag handle here (this view lays them out itself; the Overlay
 *  presentation is the one that keeps their board shape) but get a pop-out control; a child's ✕
 *  deletes it. The folder itself stays however few cards are left — emptying it is not the same as
 *  wanting it gone. */
function FolderPanel({
  folder,
  onChange,
  onDelete,
  onPopOut,
  onAddItem,
  onUpdateItem,
  onRemoveItem,
  docStatus,
  onIngested,
  onClose,
}: {
  folder: Widget;
  onChange: (id: string, patch: Partial<Widget>) => void;
  onDelete: (id: string) => void;
  onPopOut: (folderId: string, childId: string) => void;
  onAddItem: (id: string) => void;
  onUpdateItem: (id: string, itemId: string, patch: { date?: string; label?: string }) => void;
  onRemoveItem: (id: string, itemId: string) => void;
  docStatus: Map<string, DocStatus>;
  onIngested: () => void;
  onClose: () => void;
}) {
  const children = folder.children ?? [];
  // Stable, so a card body's memo isn't defeated by a fresh arrow on every render.
  const popOut = useCallback(
    (childId: string) => onPopOut(folder.id, childId),
    [onPopOut, folder.id],
  );
  return (
    <div className="flex h-full min-h-0 flex-col">
      <FolderPanelHeader folder={folder} onChange={onChange} onClose={onClose} />
      <div className="min-h-0 flex-1 overflow-auto p-3">
        {children.length === 0 ? (
          <p className="text-xs text-ink4">This folder is empty.</p>
        ) : (
          /* Always two columns. `lg:grid-cols-3` was a VIEWPORT query on a panel that is a fixed
             576px wide: on any window past 1024px it squeezed three ~175px cards in, which is what
             pushed each card's ✕ out of sight. The panel's width doesn't depend on the window, so
             neither should its column count. */
          <div className="grid grid-cols-2 gap-3">
            {children.map((c) => (
              <div
                key={c.id}
                className="flex h-64 flex-col overflow-hidden rounded-[var(--radius-sm)] border"
                style={tintStyle(c.color)}
              >
                {c.kind === "note" ? (
                  <NoteBody
                    widget={c}
                    onChange={onChange}
                    onDelete={onDelete}
                    onPopOut={popOut}
                    status={docStatus.get(`note:${c.id}`)}
                    onIngested={onIngested}
                  />
                ) : (
                  <TimelineBody
                    widget={c}
                    showPower={false}
                    onChange={onChange}
                    onDelete={onDelete}
                    onPopOut={popOut}
                    onAddItem={onAddItem}
                    onUpdateItem={onUpdateItem}
                    onRemoveItem={onRemoveItem}
                  />
                )}
              </div>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}

/** A milestone's effective date as YYYY-MM-DD, or "" when undated. */
function msDate(m: Milestone): string {
  return m.due_date?.slice(0, 10) ?? "";
}

/** A project-bound timeline: reads the project's real milestones and lays them out earliest→latest
 *  on a line. Every add/edit/remove writes straight through the milestone commands, so it stays in
 *  step with the daily brief and the project's Focus/sidebar (and refetches on window focus, so a
 *  milestone added elsewhere shows up here). */
function BoundTimeline({
  project,
  view,
  showPower,
  onUnlink,
}: {
  project: string;
  view: TimelineView;
  showPower: boolean;
  onUnlink: () => void;
}) {
  const [milestones, setMilestones] = useState<Milestone[]>([]);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [showPast, setShowPast] = useState(readShowPastTimelineItems);
  const [showCompleted, setShowCompleted] = useState(readShowCompletedTimelineItems);
  const today = todayIso();

  const refresh = useCallback(() => {
    let cancelled = false;
    listMilestones(project)
      .then((ms) => !cancelled && setMilestones(ms))
      .catch(() => !cancelled && setMilestones([]))
      .finally(() => !cancelled && setLoading(false));
    return () => {
      cancelled = true;
    };
  }, [project]);

  useEffect(() => refresh(), [refresh]);

  useEffect(() => {
    const onFocus = () => refresh();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [refresh]);

  // Earliest→latest by effective date; undated sink to the end.
  const sorted = [...milestones].sort((a, b) => {
    const da = msDate(a);
    const db = msDate(b);
    if (!da) return 1;
    if (!db) return -1;
    return da.localeCompare(db);
  });
  // Each filter is offered only when it has something to hide — on a card this size a checkbox that
  // does nothing is worse than no checkbox (the same rule the Past toggle already followed).
  const hasPast = sorted.some((m) => isPastTimelineDate(msDate(m), today));
  const hasCompleted = sorted.some((m) => m.state === "met");
  const ordered = sorted
    .filter((m) => showPast || !isPastTimelineDate(msDate(m), today))
    .filter((m) => showCompleted || m.state !== "met");

  async function add() {
    await runMutation(async () => {
      await addMilestone(project, "deadline", null, null);
      refresh();
    }, setError);
  }

  return (
    <div className="flex h-full flex-col px-2 py-1">
      <div className="mb-1 flex items-center justify-between gap-1">
        <span className="min-w-0 flex-1 truncate text-sm font-medium text-ink2" title={project}>
          {project}
        </span>
        {/* Only offered when there is something to hide — on a card this size an always-present
            checkbox that does nothing is worse than no checkbox. */}
        {hasPast && (
          <TimelineFilterToggle
            label="Past"
            title="Show entries whose date has already passed"
            checked={showPast}
            onChange={(v) => {
              setShowPast(v);
              writeShowPastTimelineItems(v);
            }}
          />
        )}
        {hasCompleted && (
          <TimelineFilterToggle
            label="Done"
            title="Show milestones already marked Done"
            checked={showCompleted}
            onChange={(v) => {
              setShowCompleted(v);
              writeShowCompletedTimelineItems(v);
            }}
          />
        )}
        <button
          onClick={onUnlink}
          title="Unlink this project (its milestones stay in the project)"
          data-help="pinboard-timeline-unlink"
          className="shrink-0 rounded-[var(--radius-sm)] px-1 text-[0.625rem] uppercase tracking-wide text-ink4 hover:text-ink2"
        >
          Unlink
        </button>
      </div>

      {loading ? (
        <p className="text-[0.6875rem] text-ink4">Loading…</p>
      ) : ordered.length === 0 ? (
        <p className="text-[0.6875rem] text-ink4">
          {milestones.length === 0
            ? "No milestones yet — add one below."
            : "Everything here has already passed — tick “Past” to see it."}
        </p>
      ) : view === "row" ? (
        <TimelineTrack>
          {ordered.map((m) => (
            <MilestoneColumn
              key={m.id}
              m={m}
              onChanged={refresh}
              onError={setError}
              showPower={showPower}
            />
          ))}
        </TimelineTrack>
      ) : (
        <TimelineList>
          {ordered.map((m) => (
            <MilestoneRow
              key={m.id}
              m={m}
              onChanged={refresh}
              onError={setError}
              showPower={showPower}
            />
          ))}
        </TimelineList>
      )}

      <button
        onClick={add}
        className="mt-1 shrink-0 self-start rounded-[var(--radius-sm)] px-1 py-0.5 text-[0.6875rem] text-accent-text hover:bg-surface"
      >
        + Milestone
      </button>
      {error && (
        <p role="alert" className="mt-1 shrink-0 px-1 text-[0.625rem] text-st-due">
          {error}
        </p>
      )}
    </div>
  );
}

/** Shared editing state + mutations for one milestone, so the column (row-view) and row (list-view)
 *  presentations stay in lock-step. Every edit persists through the milestone commands; a
 *  calendar-linked milestone keeps its synced date read-only. */
function useMilestoneEditor(
  m: Milestone,
  onChanged: () => void,
  onError: (message: string | null) => void,
) {
  const [label, setLabel] = useState(m.label);
  const [date, setDate] = useState(msDate(m));
  const met = m.state === "met";

  // Adopt fresh values when a refetch brings them in (e.g. a calendar-linked date syncing).
  useEffect(() => setLabel(m.label), [m.label]);
  useEffect(() => setDate(m.due_date?.slice(0, 10) ?? ""), [m.due_date]);

  // `dateOverride` is how DateField commits — it hands the new value straight in, because reading
  // `date` right after a `setDate` in the same tick still sees the previous render's value. Reports
  // whether the write landed, so an optimistic caller can put its field back; a no-op counts as
  // success, or a legitimate nothing-to-do would read as a refusal.
  const persist = async (dateOverride?: string): Promise<boolean> => {
    const nextLabel = label.trim() || "deadline";
    const effectiveDate = dateOverride ?? date;
    const nextDate = m.calendar_linked ? null : effectiveDate || null;
    const curDate = m.calendar_linked ? null : msDate(m) || null;
    if (nextLabel === m.label && nextDate === curDate) return true;
    return await runMutation(async () => {
      await updateMilestone(m.id, nextLabel, nextDate);
      onChanged();
    }, onError);
  };
  const setStatus = (next: MilestoneStatus) =>
    void runMutation(async () => {
      await setMilestoneStatus(m.id, next);
      onChanged();
    }, onError);
  const remove = () =>
    void runMutation(async () => {
      await deleteMilestone(m.id);
      onChanged();
    }, onError);

  return {
    label,
    setLabel,
    date,
    setDate,
    met,
    status: milestoneStatus(m),
    persist,
    setStatus,
    remove,
  };
}

interface MilestoneItemProps {
  m: Milestone;
  onChanged: () => void;
  onError: (message: string | null) => void;
  showPower: boolean;
}

/** How full a milestone's dot reads at each status. Progress is shown by FILL along one hue rather
 *  than by four different colours: the `--st-*` tokens already carry project-status meanings, and
 *  borrowing them here would say something PM doesn't mean. Done alone changes hue, to the same
 *  muted `--st-track` that has always marked a finished milestone. */
const DOT_FILL: Record<MilestoneStatus, string> = {
  not_started: "transparent",
  in_progress: "color-mix(in oklab, var(--accent) 45%, transparent)",
  almost_done: "var(--accent)",
  done: "var(--st-track)",
};

/** A milestone's status as a dot on the track — a READOUT, not a control.
 *
 *  It used to be the done toggle, which made it the second control writing the same fact as the
 *  progress dropdown. The dot stays because the track view needs a marker ON the line to show where
 *  a milestone sits; it just no longer does anything when clicked. It carries an accessible label
 *  because in the track view it is the only place the status is stated. */
function MilestoneDot({ status }: { status: MilestoneStatus }) {
  const label = MILESTONE_STATUSES.find((s) => s.value === status)?.label ?? status;
  return (
    <span
      role="img"
      aria-label={label}
      title={label}
      className="h-2.5 w-2.5 shrink-0 rounded-full border"
      style={{
        background: DOT_FILL[status],
        // Not-started reads as an outline, so its ring has to be the accent rather than the panel
        // colour it would otherwise vanish against.
        borderColor: status === "not_started" ? "var(--accent)" : "var(--panel)",
      }}
    />
  );
}

/** One milestone as a column on the row/track view: date on top, a dot on the line, its label
 *  below, and a remove ✕. Calendar-linked milestones show their synced date read-only.
 *
 *  Status is READ-ONLY here. A four-option dropdown does not fit an 5.5rem column, and shortening
 *  the labels to make it fit would mean the same control reading differently in two places. The
 *  track is the overview; switch the card to list view to change a status. */
function MilestoneColumn({ m, onChanged, onError, showPower }: MilestoneItemProps) {
  const { label, setLabel, date, setDate, met, status, persist, remove } = useMilestoneEditor(
    m,
    onChanged,
    onError,
  );
  return (
    <div className="flex w-[5.5rem] shrink-0 flex-col items-center gap-1 text-center">
      {m.calendar_linked ? (
        <span
          className="flex h-6 items-center font-mono text-[0.5625rem] text-accent-text"
          title={
            m.event_missing ? "Linked event not found in your calendars" : "Synced from calendar"
          }
        >
          📅 {m.due_date ? formatDateOnly(msDate(m)) : "—"}
        </span>
      ) : (
        <DateField
          value={date}
          onCommit={(iso) => {
            const prev = date;
            setDate(iso);
            // A refused write triggers no refetch, so `m.due_date` never changes and the adopt
            // effect cannot undo this — roll the field back instead of leaving it showing, and
            // later silently re-committing, a date the backend rejected.
            void persist(iso).then((ok) => {
              if (!ok) setDate(prev);
            });
          }}
          ariaLabel="Milestone deadline"
          wrapperClassName="w-full"
          className="h-6 px-0.5 font-mono text-[0.5625rem] text-ink3"
        />
      )}
      <MilestoneDot status={status} />
      <input
        value={label}
        onChange={(e) => setLabel(e.target.value)}
        onBlur={() => void persist()}
        placeholder="label"
        className={`w-full rounded-[var(--radius-sm)] bg-transparent px-0.5 text-center text-[0.625rem] text-ink2 focus:outline-none ${
          met ? "text-ink4 line-through" : ""
        }`}
      />
      <button
        onClick={remove}
        aria-label="Remove milestone"
        className="inline-flex min-h-[var(--tap-min,24px)] min-w-[var(--tap-min,24px)] items-center justify-center text-[0.625rem] text-ink4 hover:text-st-due"
      >
        ✕
      </button>
      {showPower && m.event_missing && (
        <span className="text-[0.5625rem] text-st-due">⚠ unsynced</span>
      )}
    </div>
  );
}

/** One milestone as a list row: date · label · progress · remove — the same edits as the column
 *  view, laid out horizontally, plus the progress dropdown the narrow track can't hold. This is
 *  where a status is CHANGED.
 *
 *  No status dot here. The track view keeps one because it is the marker showing where a milestone
 *  sits on the line and the only place its status is stated; in a list row the dropdown right there
 *  already says it in words, so the dot was a second, vaguer copy of the same fact. */
function MilestoneRow({ m, onChanged, onError, showPower }: MilestoneItemProps) {
  const { label, setLabel, date, setDate, met, status, persist, setStatus, remove } =
    useMilestoneEditor(m, onChanged, onError);
  return (
    <div className="flex items-center gap-1">
      {m.calendar_linked ? (
        <span
          className="flex h-6 w-[6.25rem] shrink-0 items-center gap-0.5 font-mono text-[0.5625rem] text-accent-text"
          title={
            m.event_missing ? "Linked event not found in your calendars" : "Synced from calendar"
          }
        >
          📅 {m.due_date ? formatDateOnly(msDate(m)) : "—"}
        </span>
      ) : (
        <DateField
          value={date}
          onCommit={(iso) => {
            const prev = date;
            setDate(iso);
            // Same rollback as the column view above: no refetch follows a refusal, so the adopt
            // effect never fires and the field would keep — and re-commit — the rejected date.
            void persist(iso).then((ok) => {
              if (!ok) setDate(prev);
            });
          }}
          ariaLabel="Milestone deadline"
          wrapperClassName="w-[7.5rem] shrink-0"
          className="px-1 py-0.5 font-mono text-[0.625rem] text-ink3"
        />
      )}
      <input
        value={label}
        onChange={(e) => setLabel(e.target.value)}
        onBlur={() => void persist()}
        placeholder="label"
        className={`min-w-0 flex-1 rounded-[var(--radius-sm)] border border-transparent bg-transparent px-1 py-0.5 text-xs text-ink2 focus:border-accent focus:outline-none ${
          met ? "text-ink4 line-through" : ""
        }`}
      />
      {showPower && m.event_missing && (
        <span className="shrink-0 text-[0.5625rem] text-st-due" title="Linked event not found">
          ⚠
        </span>
      )}
      <Select
        compact
        value={status}
        aria-label={`Progress for ${m.label || "milestone"}`}
        title="How far along this milestone is"
        onChange={(e) => setStatus(e.target.value as MilestoneStatus)}
        className="shrink-0 text-[0.625rem]"
      >
        {MILESTONE_STATUSES.map((s) => (
          <option key={s.value} value={s.value}>
            {s.label}
          </option>
        ))}
      </Select>
      <button
        onClick={remove}
        aria-label="Remove milestone"
        className="inline-flex min-h-[var(--tap-min,24px)] min-w-[var(--tap-min,24px)] shrink-0 items-center justify-center text-[0.6875rem] text-ink4 hover:text-st-due"
      >
        ✕
      </button>
    </div>
  );
}

/** The default freeform timeline: type-in dated entries that live in the widget, shown as a list or
 *  a horizontal track, plus a picker to bind the card to a real project (which switches it to
 *  {@link BoundTimeline}). */
function FreeformTimeline({
  widget,
  view,
  onChange,
  onAddItem,
  onUpdateItem,
  onRemoveItem,
}: TimelineBodyProps & { view: TimelineView }) {
  const [projects, setProjects] = useState<string[]>([]);
  const [projDraft, setProjDraft] = useState("");
  const [linkErr, setLinkErr] = useState<string | null>(null);
  const [showPast, setShowPast] = useState(readShowPastTimelineItems);
  const today = todayIso();
  useEffect(() => {
    listProjects()
      .then(setProjects)
      .catch(() => setProjects([]));
  }, []);
  const listId = `pm-projects-${widget.id}`;

  // Show items in date order; undated items sink to the bottom.
  const sortedItems = [...(widget.items ?? [])].sort((a, b) => {
    if (!a.date) return 1;
    if (!b.date) return -1;
    return a.date.localeCompare(b.date);
  });
  const hasPast = sortedItems.some((it) => isPastTimelineDate(it.date, today));
  const items = showPast
    ? sortedItems
    : sortedItems.filter((it) => !isPastTimelineDate(it.date, today));

  // Bind to a project — but first MERGE the freeform entries into that project's real milestones, so
  // what you typed becomes the project's milestones instead of vanishing behind the bound view. Dedup
  // against what's already there (by label + date) so re-linking after an Unlink can't duplicate. A
  // new project name is fine — addMilestone creates the project on first insert.
  //
  // The merge CONSUMES the entries, so they're cleared in the same commit: each one is now a real
  // milestone (or already matched one), so nothing is lost — and a widget that kept a private copy
  // would re-emit it onto the calendar's pinboard overlay the moment it was unlinked, drawing that
  // deadline twice (once as the entry, once as the milestone it became). Both writes land together,
  // after the milestones are safely persisted, so a failed merge leaves the entries untouched.
  async function linkProject(project: string) {
    await runMutation(async () => {
      const existing = await listMilestones(project);
      const seen = new Set(
        existing.map((m) => `${m.label.trim()} ${m.due_date?.slice(0, 10) ?? ""}`),
      );
      for (const it of widget.items ?? []) {
        const label = it.label.trim() || "deadline";
        const date = it.date || null;
        const key = `${label} ${date ?? ""}`;
        if (seen.has(key)) continue;
        seen.add(key);
        await addMilestone(project, label, date, null);
      }
      onChange(widget.id, { project, items: [] });
    }, setLinkErr);
  }

  return (
    <div className="flex h-full flex-col px-2 py-1">
      {hasPast && (
        <div className="mb-1 flex shrink-0 justify-end">
          {/* Freeform entries have no notion of "done", so this card gets the date filter only. */}
          <TimelineFilterToggle
            label="Past"
            title="Show entries whose date has already passed"
            checked={showPast}
            onChange={(v) => {
              setShowPast(v);
              writeShowPastTimelineItems(v);
            }}
          />
        </div>
      )}
      {items.length === 0 ? (
        <div className="min-h-0 flex-1">
          <p className="text-[0.6875rem] text-ink4">
            {sortedItems.length === 0
              ? "No milestones yet."
              : "Everything here has already passed — tick “Past” to see it."}
          </p>
        </div>
      ) : view === "row" ? (
        <TimelineTrack>
          {items.map((it) => (
            <FreeformColumn
              key={it.id}
              widgetId={widget.id}
              item={it}
              onUpdateItem={onUpdateItem}
              onRemoveItem={onRemoveItem}
            />
          ))}
        </TimelineTrack>
      ) : (
        <TimelineList>
          {items.map((it) => (
            <div key={it.id} className="flex items-center gap-1">
              <DateField
                value={it.date ?? ""}
                onCommit={(iso) => onUpdateItem(widget.id, it.id, { date: iso })}
                ariaLabel="Timeline date"
                title={it.date ? formatDateOnly(it.date) : "Set a date"}
                wrapperClassName="w-[7.5rem] shrink-0"
                className="px-1 py-0.5 font-mono text-[0.625rem] text-ink3"
              />
              <input
                value={it.label ?? ""}
                onChange={(e) => onUpdateItem(widget.id, it.id, { label: e.target.value })}
                placeholder="what happens"
                className="min-w-0 flex-1 rounded-[var(--radius-sm)] border border-transparent bg-transparent px-1 py-0.5 text-xs text-ink2 focus:border-accent focus:outline-none"
              />
              <button
                onClick={() => onRemoveItem(widget.id, it.id)}
                aria-label="Remove milestone"
                className="inline-flex min-h-[var(--tap-min,24px)] min-w-[var(--tap-min,24px)] shrink-0 items-center justify-center text-[0.6875rem] text-ink4 hover:text-st-due"
              >
                ✕
              </button>
            </div>
          ))}
        </TimelineList>
      )}
      {/* Add-a-milestone and bind-to-a-project share one row: + Milestone on the left, the project
          picker filling the middle, Link on the right. Linking first merges the freeform entries above
          into that project's milestones (so they don't vanish behind the bound view), then switches to
          it. Typing a new name is allowed — the project is created when the first milestone is added. */}
      <div className="mt-1 flex shrink-0 items-center gap-1" data-help="pinboard-timeline-project">
        <button
          onClick={() => onAddItem(widget.id)}
          className="shrink-0 rounded-[var(--radius-sm)] px-1 py-0.5 text-[0.6875rem] text-accent-text hover:bg-surface"
        >
          + Milestone
        </button>
        <input
          list={listId}
          value={projDraft}
          onChange={(e) => setProjDraft(e.target.value)}
          placeholder="Link a project…"
          className="min-w-0 flex-1 rounded-[var(--radius-sm)] border border-border2 bg-surface px-1 py-0.5 text-[0.6875rem] text-ink3 focus:border-accent focus:outline-none"
        />
        <datalist id={listId}>
          {projects.map((p) => (
            <option key={p} value={p} />
          ))}
        </datalist>
        <button
          onClick={() => projDraft.trim() && void linkProject(projDraft.trim())}
          disabled={!projDraft.trim()}
          className="shrink-0 rounded-[var(--radius-sm)] px-1 py-0.5 text-[0.6875rem] text-accent-text hover:bg-surface disabled:opacity-40"
        >
          Link
        </button>
      </div>
      {/* Dated entries ride the calendar's pinboard overlay by default; this opts THIS timeline out.
          Deliberately freeform-only: once a timeline is linked to a project its entries are real
          milestones, and the calendar's own Milestones toggle governs them — so a second, conflicting
          switch here would just be a lie. */}
      <label className="mt-1 flex shrink-0 cursor-pointer items-center gap-1 text-[0.625rem] text-ink4">
        <input
          type="checkbox"
          checked={widget.showOnCalendar !== false}
          onChange={(e) => onChange(widget.id, { showOnCalendar: e.target.checked })}
          className="accent-[var(--accent)]"
        />
        Show on calendar
      </label>
      {linkErr && <p className="mt-1 shrink-0 text-[0.625rem] text-st-due">{linkErr}</p>}
    </div>
  );
}

/** One freeform entry as a track column: date on top, a dot on the line, its label below, remove ✕
 *  — the row-view counterpart of the list row above. */
function FreeformColumn({
  widgetId,
  item,
  onUpdateItem,
  onRemoveItem,
}: {
  widgetId: string;
  item: TimelineItem;
  onUpdateItem: (id: string, itemId: string, patch: { date?: string; label?: string }) => void;
  onRemoveItem: (id: string, itemId: string) => void;
}) {
  return (
    <div className="flex w-[5.5rem] shrink-0 flex-col items-center gap-1 text-center">
      <DateField
        value={item.date ?? ""}
        onCommit={(iso) => onUpdateItem(widgetId, item.id, { date: iso })}
        ariaLabel="Timeline date"
        title={item.date ? formatDateOnly(item.date) : "Set a date"}
        wrapperClassName="w-full"
        className="h-6 px-0.5 font-mono text-[0.5625rem] text-ink3"
      />
      <span
        className="h-2.5 w-2.5 shrink-0 rounded-full border"
        style={{ background: "var(--accent)", borderColor: "var(--panel)" }}
      />
      <input
        value={item.label ?? ""}
        onChange={(e) => onUpdateItem(widgetId, item.id, { label: e.target.value })}
        placeholder="what happens"
        className="w-full rounded-[var(--radius-sm)] bg-transparent px-0.5 text-center text-[0.625rem] text-ink2 focus:outline-none"
      />
      <button
        onClick={() => onRemoveItem(widgetId, item.id)}
        aria-label="Remove milestone"
        className="inline-flex min-h-[var(--tap-min,24px)] min-w-[var(--tap-min,24px)] items-center justify-center text-[0.625rem] text-ink4 hover:text-st-due"
      >
        ✕
      </button>
    </div>
  );
}
