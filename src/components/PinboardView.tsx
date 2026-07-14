// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import {
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
  setMilestoneState,
  updateMilestone,
} from "../lib/ipc";
import { Markdown } from "../lib/markdown";
import { CELL, COLS, ROWS, boundsForPx, minSize, pxRectToCells } from "../lib/pinboard/grid";
import {
  applyLineMarker,
  continueList,
  indentLines,
  listIndentBeforeCaret,
  outdentLines,
  toRenderMarkdown,
  toggleWrap,
  type TextEdit,
} from "../lib/pinboard/notesMarkdown";
import { usePinboard } from "../lib/pinboard/usePinboard";
// The tint set + names live in one place (src/lib/pinboard/palette.ts) so the board's colours
// stay consistent; the colour VALUES are the global `--st-*` tokens in index.css.
import { NOTE_COLORS, TINT_NAME } from "../lib/pinboard/palette";
import type { Rect, TimelineItem, Widget, WidgetKind } from "../lib/pinboard/types";
import type { Milestone } from "../lib/types";
import { useDepth } from "../theme";
import { Button, Modal, SegmentedControl, Textarea, Tooltip } from "./ui";

/** The live filing state of a note's ingested document, keyed by `note:<widgetId>`. */
type DocStatus = { reviewed: boolean; project: string };

/** A cheap, stable string hash (djb2) — just to tell whether a note's text has changed since it
 *  was last ingested. Not cryptographic and not the backend content hash. */
function cheapHash(s: string): string {
  let h = 5381;
  for (let i = 0; i < s.length; i++) h = (((h << 5) + h) ^ s.charCodeAt(i)) | 0;
  return (h >>> 0).toString(36);
}

interface PxRect {
  x: number;
  y: number;
  w: number;
  h: number;
}

interface DragStart {
  id: string;
  /** The dragged widget's kind, captured at grab so the drop can pick the right min size (folders
   *  stay 3×3) without the pointer effect needing to look the widget up. */
  kind: WidgetKind;
  mode: "move" | "resize";
  startX: number;
  startY: number;
  startRect: Rect;
}

function rectToPx(r: Rect): PxRect {
  return { x: r.x * CELL, y: r.y * CELL, w: r.w * CELL, h: r.h * CELL };
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

/**
 * The Pinboard (spec §4): a bounded planning board of draggable, resizable widgets —
 * post-it notes and simple dated timelines — persisted locally. Hand-rolled on pointer
 * events + CSS transforms with grid-snap (no layout library); the snap/clamp maths live in
 * `lib/pinboard/grid.ts`. The board grows to fill the window (cell size and fonts fixed) and
 * scrolls both axes once the window is made smaller. Notes and timelines are available at every
 * depth; per-widget metadata shows at `power`. Notes are Markdown: they render in place and turn
 * back into an editor on click, with a formatting toolbar, keyboard shortcuts, and smart list
 * continuation (`lib/pinboard/notesMarkdown.ts`).
 */
export function PinboardView() {
  const { showMeta, showPower } = useDepth();
  // The board is a FIXED-WIDTH canvas = the window's own content area: adding notes wraps to a new
  // row and the board only ever grows DOWNWARD (and scrolls), never sideways.
  const scrollRef = useRef<HTMLDivElement>(null);
  const [viewBounds, setViewBounds] = useState({ cols: COLS, rows: ROWS });
  // Measure the scroller's content area and track it LIVE (grow AND shrink) so the board width
  // always equals the window — not a stale high-water mark that would overflow horizontally. Layout
  // effect so the real width is known before the async board load re-flows overflow into it.
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const PAD = 24; // matches the scroller's p-6, so the board never forces a horizontal scrollbar
    const measure = () =>
      setViewBounds({
        cols: Math.max(COLS, Math.floor((el.clientWidth - PAD * 2) / CELL)),
        rows: Math.max(ROWS, Math.floor((el.clientHeight - PAD * 2) / CELL)),
      });
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  // A generous vertical extent (the device screen height, fixed per machine) so notes wrap to new
  // rows with room to spare before the board scrolls; the WIDTH is always the live window, so the
  // board never widens. These are the bounds the hook places, drops, and re-flows within.
  const screenRows = useMemo(
    () => boundsForPx({ w: window.screen.availWidth, h: window.screen.availHeight }).rows,
    [],
  );
  const boardConstraints = useMemo(
    () => ({ cols: viewBounds.cols, rows: Math.max(viewBounds.rows, screenRows) }),
    [viewBounds.cols, viewBounds.rows, screenRows],
  );
  const {
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
  } = usePinboard(boardConstraints);

  const [drag, setDrag] = useState<DragStart | null>(null);
  const [livePx, setLivePx] = useState<PxRect | null>(null);
  // Which folder is currently expanded (transient — not persisted). At most one at a time.
  const [expandedFolderId, setExpandedFolderId] = useState<string | null>(null);
  // A just-added widget id to scroll into view (set by the add buttons; cleared once scrolled).
  const [pendingScrollId, setPendingScrollId] = useState<string | null>(null);

  // Width is the fixed window (viewBounds.cols); the board grows only DOWNWARD to contain content
  // (vertical scroll). We still take the max with widget extents defensively — so that if the window
  // is dragged narrower than existing content the widgets stay reachable until the next load re-flows
  // them in — but new notes are always placed within the window, so adding them never widens it.
  const boardBounds = useMemo(() => {
    let cols = viewBounds.cols;
    let rows = viewBounds.rows;
    for (const w of board.widgets) {
      cols = Math.max(cols, w.rect.x + w.rect.w);
      rows = Math.max(rows, w.rect.y + w.rect.h);
    }
    return { cols, rows };
  }, [viewBounds, board.widgets]);
  // Read board bounds through a ref inside the drag effect so an in-flight drag always clamps to the
  // current board without the effect re-subscribing to pointer events on every resize.
  const boardBoundsRef = useRef(boardBounds);
  boardBoundsRef.current = boardBounds;

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

  // While a drag is active, track the pointer on the window so it keeps following even when
  // the cursor leaves the widget; commit the snapped cell rect on release. Effect re-runs only
  // when a drag starts/ends (livePx lives in its own state, off the dependency list).
  useEffect(() => {
    if (!drag) return;
    const startPx = rectToPx(drag.startRect);
    const compute = (e: PointerEvent): PxRect => {
      const dx = e.clientX - drag.startX;
      const dy = e.clientY - drag.startY;
      const maxX = boardBoundsRef.current.cols * CELL;
      const maxY = boardBoundsRef.current.rows * CELL;
      if (drag.mode === "move") {
        return {
          x: Math.max(0, Math.min(startPx.x + dx, maxX - startPx.w)),
          y: Math.max(0, Math.min(startPx.y + dy, maxY - startPx.h)),
          w: startPx.w,
          h: startPx.h,
        };
      }
      // Clamp to the kind's own minimum so a folder can shrink back to its 3×3 floor (not the
      // note/timeline 4×3) while still growing freely.
      const min = minSize(drag.kind);
      return {
        x: startPx.x,
        y: startPx.y,
        w: Math.max(min.w * CELL, Math.min(startPx.w + dx, maxX - startPx.x)),
        h: Math.max(min.h * CELL, Math.min(startPx.h + dy, maxY - startPx.y)),
      };
    };
    const onMove = (e: PointerEvent) => setLivePx(compute(e));
    const onUp = (e: PointerEvent) => {
      const rect = pxRectToCells(
        compute(e),
        boardBoundsRef.current.cols,
        boardBoundsRef.current.rows,
        minSize(drag.kind),
      );
      // Resize just repositions; a move goes through dropWidget, which may fold two stacked widgets
      // into a folder or drop one into an existing folder.
      if (drag.mode === "resize") moveWidget(drag.id, rect);
      else dropWidget(drag.id, rect);
      setDrag(null);
      setLivePx(null);
    };
    // If the gesture is interrupted (touch handed to the scroller, an OS context menu,
    // the window losing focus), the browser fires pointercancel/blur instead of pointerup.
    // Without this the drag would dangle: `drag` stuck non-null, `select-none` stuck on, and
    // the move silently lost. pointercancel still carries coordinates so we commit; a blur
    // has none, so we just end the drag cleanly.
    const onCancel = (e: PointerEvent) => onUp(e);
    const onBlur = () => {
      setDrag(null);
      setLivePx(null);
    };
    window.addEventListener("pointermove", onMove);
    window.addEventListener("pointerup", onUp);
    window.addEventListener("pointercancel", onCancel);
    window.addEventListener("blur", onBlur);
    return () => {
      window.removeEventListener("pointermove", onMove);
      window.removeEventListener("pointerup", onUp);
      window.removeEventListener("pointercancel", onCancel);
      window.removeEventListener("blur", onBlur);
    };
  }, [drag, moveWidget, dropWidget]);

  // Stable across the per-tick drag re-renders (only `raiseWidget` is a dep), so passing it into each
  // memoised body doesn't defeat the memo that keeps react-markdown from re-running on every move.
  const startDrag = useCallback(
    (e: ReactPointerEvent, w: Widget, mode: "move" | "resize") => {
      e.preventDefault();
      raiseWidget(w.id);
      setDrag({
        id: w.id,
        kind: w.kind,
        mode,
        startX: e.clientX,
        startY: e.clientY,
        startRect: w.rect,
      });
      setLivePx(rectToPx(w.rect));
    },
    [raiseWidget],
  );

  // Add buttons remember the new widget so it can be scrolled into view (a note placed on a lower
  // row would otherwise be created off-screen).
  const handleAddNote = useCallback(() => setPendingScrollId(addNote()), [addNote]);
  const handleAddTimeline = useCallback(() => setPendingScrollId(addTimeline()), [addTimeline]);
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
        el.scrollTo({ top: nextTop, left: nextLeft, behavior: "smooth" });
      }
    }
    setPendingScrollId(null);
  }, [pendingScrollId, board.widgets]);

  // If the expanded folder dissolves (its last child removed/popped), close the panel.
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

  return (
    // min-w-0 / min-h-0 keep the oversized board penned inside its own overflow-auto scroller
    // (below) instead of inflating this column and pushing the header's buttons off-screen.
    <div className="flex h-full min-w-0 min-h-0 flex-1 flex-col">
      <header className="flex items-center justify-between gap-3 border-b border-rule px-6 py-3">
        <div>
          <h1 className="font-head text-lg font-semibold text-ink">Pinboard</h1>
          {showMeta && (
            <p className="text-xs text-ink4">
              A space to think — drag to arrange, resize from the corner. Saved on this device.
            </p>
          )}
        </div>
        <div className="flex items-center gap-2">
          {showPower && board.widgets.length > 0 && (
            <span className="mr-1 font-mono text-[10px] uppercase tracking-wide text-faint">
              {board.widgets.length} item{board.widgets.length === 1 ? "" : "s"}
            </span>
          )}
          <Button
            variant="secondary"
            onClick={handleAddNote}
            className="px-2.5 py-1 text-xs"
            data-help="pinboard-add-note"
          >
            + Note
          </Button>
          {/* Notes and timelines are both available at every density. */}
          <Button
            variant="secondary"
            onClick={handleAddTimeline}
            className="px-2.5 py-1 text-xs"
            data-help="pinboard-add-timeline"
          >
            + Timeline
          </Button>
        </div>
      </header>

      <div ref={scrollRef} className="pm-scrollbars min-h-0 min-w-0 flex-1 overflow-auto p-6">
        <div
          data-help="pinboard-board"
          className={`relative rounded-[var(--radius)] border border-border ${
            drag ? "select-none" : ""
          }`}
          style={{
            width: boardBounds.cols * CELL,
            height: boardBounds.rows * CELL,
            backgroundColor: "var(--surface)",
            backgroundImage:
              "linear-gradient(var(--rule) 1px, transparent 1px), linear-gradient(90deg, var(--rule) 1px, transparent 1px)",
            backgroundSize: `${CELL}px ${CELL}px`,
          }}
        >
          {board.widgets.length === 0 && (
            <div className="pointer-events-none absolute inset-0 flex items-center justify-center">
              <p className="text-sm text-ink4">
                Add a note to start planning — it stays here between visits.
              </p>
            </div>
          )}

          {board.widgets.map((w) => {
            const px = drag?.id === w.id && livePx ? livePx : rectToPx(w.rect);
            return (
              <div
                key={w.id}
                data-help={
                  w.kind === "note"
                    ? "pinboard-note"
                    : w.kind === "timeline"
                      ? "pinboard-timeline"
                      : "pinboard-folder"
                }
                className="absolute flex flex-col overflow-hidden rounded-[var(--radius-sm)] border shadow-sm transition-shadow hover:shadow-md motion-reduce:transition-none"
                style={{ left: px.x, top: px.y, width: px.w, height: px.h, ...tintStyle(w.color) }}
              >
                {/* The widget owns its own header + body, so note-specific controls (Ingest) can sit
                    in the top bar while their state stays inside the memoised body. */}
                {w.kind === "note" ? (
                  <NoteBody
                    widget={w}
                    showPower={showPower}
                    onChange={updateWidget}
                    onDelete={removeWidget}
                    onStartDrag={startDrag}
                    status={docStatus.get(`note:${w.id}`)}
                    onIngested={refreshDocs}
                  />
                ) : w.kind === "timeline" ? (
                  <TimelineBody
                    widget={w}
                    showPower={showPower}
                    onChange={updateWidget}
                    onDelete={removeWidget}
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

                {/* A note shows its size inline in its own footer (see NoteBody); timeline/folder
                    keep the compact coords strip. */}
                {showPower && w.kind !== "note" && (
                  <div className="shrink-0 border-t border-rule px-2 py-0.5 font-mono text-[9px] text-faint">
                    {w.rect.x},{w.rect.y} · {w.rect.w}×{w.rect.h}
                  </div>
                )}

                {/* Resize handle (bottom-right) — every widget kind is resizable (folders floor at 3×3). */}
                <div
                  onPointerDown={(e) => startDrag(e, w, "resize")}
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
          })}

          {/* Expanded folder (transient UI, at most one). Rendered as a board sibling so the inline
              panel escapes the tiles' overflow-hidden and paints above them; the tile's rect never moves. */}
          {expandedFolder &&
            ((expandedFolder.expandMode ?? "inline") === "overlay" ? (
              <Modal open onClose={() => setExpandedFolderId(null)} widthClassName="max-w-3xl">
                <div className="h-[70vh]">
                  <FolderPanel
                    folder={expandedFolder}
                    onChange={updateWidget}
                    onDelete={removeWidget}
                    onPopOut={popOutChild}
                    onAddItem={addTimelineItem}
                    onUpdateItem={updateTimelineItem}
                    onRemoveItem={removeTimelineItem}
                    docStatus={docStatus}
                    onIngested={refreshDocs}
                    onClose={() => setExpandedFolderId(null)}
                  />
                </div>
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
                  onDelete={removeWidget}
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
        </div>
      </div>
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
    icon: <span className="text-[11px] font-bold leading-none">B</span>,
    apply: (v, s, e) => toggleWrap(v, s, e, "**"),
  },
  {
    key: "italic",
    label: "Italic",
    hint: `${MOD}I`,
    icon: <span className="font-serif text-[11px] italic leading-none">I</span>,
    apply: (v, s, e) => toggleWrap(v, s, e, "*"),
  },
  {
    key: "heading",
    label: "Heading",
    hint: `${MOD}${SHIFT}H`,
    icon: <span className="text-[11px] font-bold leading-none">H</span>,
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
          (mirrors the ✕ button). The title now grows to fill the bar (nearly up to the actions),
          and a fixed drag spacer to its right keeps a reliable grab zone even for a long title. */}
      <input
        value={widget.title ?? ""}
        onChange={(e) => onRename(e.target.value)}
        onPointerDown={(e) => e.stopPropagation()}
        placeholder={placeholder}
        aria-label={`${placeholder} title`}
        className="min-w-0 flex-1 truncate border-0 bg-transparent px-0 text-xs font-medium text-ink3 placeholder:text-ink4 focus:text-ink2 focus:outline-none focus:ring-0"
      />
      {/* An always-present drag grip so a long title still leaves somewhere to grab the header. */}
      <div className="w-6 shrink-0 self-stretch" aria-hidden="true" />
      <div className="flex shrink-0 items-center gap-1">
        {actions}
        <button
          onPointerDown={(e) => e.stopPropagation()}
          onClick={onDelete}
          aria-label={deleteLabel}
          title={deleteLabel}
          className="shrink-0 rounded-[var(--radius-sm)] px-1 text-xs text-ink4 hover:bg-surface hover:text-st-due"
        >
          ✕
        </button>
      </div>
    </div>
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
      className="shrink-0 rounded-[var(--radius-sm)] px-1 text-xs text-ink4 hover:bg-surface hover:text-ink2"
    >
      ⤴
    </button>
  );
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
  /** When set (folder-panel child), a "move out to the board" control shows in the header. */
  onPopOut?: () => void;
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
      await ingestNote(widget.id, toRenderMarkdown(text));
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
      onChange(widget.id, { text: res.text });
      setEditing(true);
      requestAnimationFrame(() => {
        const t = taRef.current;
        if (!t) return;
        t.focus();
        t.selectionStart = res.selStart;
        t.selectionEnd = res.selEnd;
      });
    },
    [text, onChange, widget.id],
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
    onChange(widget.id, { text: res.text });
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
      className="shrink-0 rounded-[var(--radius-sm)] px-1 text-[10px] uppercase tracking-wide text-accent-text hover:bg-surface disabled:opacity-40"
      title="Save this note to your vault as a document (it goes through Review)"
    >
      {ingesting ? "Saving…" : "Ingest"}
    </button>
  ) : (
    <>
      <span
        className="max-w-[8rem] truncate text-[10px] text-ink4"
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
          className="shrink-0 rounded-[var(--radius-sm)] px-1 text-[10px] uppercase tracking-wide text-accent-text hover:bg-surface disabled:opacity-40"
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
            {onPopOut && <PopOutButton onClick={onPopOut} />}
            {ingestControl}
          </>
        }
      />
      {ingestErr && <p className="shrink-0 px-2 pt-1 text-[10px] text-st-due">{ingestErr}</p>}
      {showEditor ? (
        <Textarea
          ref={taRef}
          value={text}
          onChange={(e) => onChange(widget.id, { text: e.target.value })}
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
          onClick={() => setEditing(true)}
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
                    className="flex h-5 w-5 items-center justify-center rounded-[var(--radius-sm)] text-ink4 hover:bg-surface hover:text-ink2"
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
              <span className="font-mono text-[9px] text-faint">
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
  /** The board drag handle. Absent for folder-panel children (they aren't board-positioned). */
  onStartDrag?: (e: ReactPointerEvent, w: Widget, mode: "move" | "resize") => void;
  /** When set (folder-panel child), a "move out to the board" control shows in the header. */
  onPopOut?: () => void;
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
            {onPopOut && <PopOutButton onClick={onPopOut} />}
          </>
        }
      />
      <div className="min-h-0 flex-1 overflow-auto">
        {widget.project ? (
          <BoundTimeline
            project={widget.project}
            view={view}
            showPower={showPower}
            onUnlink={() => onChange(widget.id, { project: undefined })}
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
      className={`flex h-5 w-5 items-center justify-center rounded-[var(--radius-sm)] ${
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

/** A collapsed folder tile (3×3): the shared header (editable title + Ungroup) over a big button
 *  showing the child count, which opens the folder. Not resizable (the outer loop omits the handle). */
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
        <span className="font-mono text-[10px]">
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

/** The expanded folder view (inline panel or overlay): editable title, a presentation toggle, and
 *  a grid of the contained cards — each the SAME NoteBody/TimelineBody, so children edit/ingest just
 *  like board widgets. Children carry no drag handle (they aren't board-positioned) but get a
 *  pop-out control; a child's ✕ deletes it (and auto-dissolves the folder at ≤1). */
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
  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex shrink-0 items-center justify-between gap-2 border-b border-rule px-3 py-2">
        <input
          value={folder.title ?? ""}
          onChange={(e) => onChange(folder.id, { title: e.target.value })}
          placeholder="Folder"
          aria-label="Folder title"
          className="min-w-0 flex-1 truncate border-0 bg-transparent px-0 text-sm font-medium text-ink2 placeholder:text-ink4 focus:outline-none focus:ring-0"
        />
        <SegmentedControl
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
          className="shrink-0 rounded-[var(--radius-sm)] px-1 text-sm text-ink4 hover:bg-surface hover:text-ink2"
        >
          ✕
        </button>
      </div>
      <div className="min-h-0 flex-1 overflow-auto p-3">
        {children.length === 0 ? (
          <p className="text-xs text-ink4">This folder is empty.</p>
        ) : (
          <div className="grid grid-cols-2 gap-3 lg:grid-cols-3">
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
                    onPopOut={() => onPopOut(folder.id, c.id)}
                    status={docStatus.get(`note:${c.id}`)}
                    onIngested={onIngested}
                  />
                ) : (
                  <TimelineBody
                    widget={c}
                    showPower={false}
                    onChange={onChange}
                    onDelete={onDelete}
                    onPopOut={() => onPopOut(folder.id, c.id)}
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
  const ordered = [...milestones].sort((a, b) => {
    const da = msDate(a);
    const db = msDate(b);
    if (!da) return 1;
    if (!db) return -1;
    return da.localeCompare(db);
  });

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
        <button
          onClick={onUnlink}
          title="Unlink this project (its milestones stay in the project)"
          data-help="pinboard-timeline-unlink"
          className="shrink-0 rounded-[var(--radius-sm)] px-1 text-[10px] uppercase tracking-wide text-ink4 hover:text-ink2"
        >
          Unlink
        </button>
      </div>

      {loading ? (
        <p className="text-[11px] text-ink4">Loading…</p>
      ) : ordered.length === 0 ? (
        <p className="text-[11px] text-ink4">No milestones yet — add one below.</p>
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
        className="mt-1 shrink-0 self-start rounded-[var(--radius-sm)] px-1 py-0.5 text-[11px] text-accent-text hover:bg-surface"
      >
        + Milestone
      </button>
      {error && (
        <p role="alert" className="mt-1 shrink-0 px-1 text-[10px] text-st-due">
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

  const persist = async () => {
    const nextLabel = label.trim() || "deadline";
    const nextDate = m.calendar_linked ? null : date || null;
    const curDate = m.calendar_linked ? null : msDate(m) || null;
    if (nextLabel === m.label && nextDate === curDate) return;
    await runMutation(async () => {
      await updateMilestone(m.id, nextLabel, nextDate);
      onChanged();
    }, onError);
  };
  const toggleDone = () =>
    void runMutation(async () => {
      await setMilestoneState(m.id, !met);
      onChanged();
    }, onError);
  const remove = () =>
    void runMutation(async () => {
      await deleteMilestone(m.id);
      onChanged();
    }, onError);

  return { label, setLabel, date, setDate, met, persist, toggleDone, remove };
}

interface MilestoneItemProps {
  m: Milestone;
  onChanged: () => void;
  onError: (message: string | null) => void;
  showPower: boolean;
}

/** A milestone's done toggle — a dot that reads "done" (st-track) or "open" (accent). */
function MilestoneDot({ met, onToggle }: { met: boolean; onToggle: () => void }) {
  return (
    <button
      onClick={onToggle}
      title={met ? "Mark not done" : "Mark done"}
      aria-label={met ? "Mark not done" : "Mark done"}
      className="h-2.5 w-2.5 shrink-0 rounded-full border"
      style={{ background: met ? "var(--st-track)" : "var(--accent)", borderColor: "var(--panel)" }}
    />
  );
}

/** One milestone as a column on the row/track view: date on top, a dot on the line, its label
 *  below, and a remove ✕. Calendar-linked milestones show their synced date read-only. */
function MilestoneColumn({ m, onChanged, onError, showPower }: MilestoneItemProps) {
  const { label, setLabel, date, setDate, met, persist, toggleDone, remove } = useMilestoneEditor(
    m,
    onChanged,
    onError,
  );
  return (
    <div className="flex w-[5.5rem] shrink-0 flex-col items-center gap-1 text-center">
      {m.calendar_linked ? (
        <span
          className="flex h-6 items-center font-mono text-[9px] text-accent-text"
          title={
            m.event_missing ? "Linked event not found in your calendars" : "Synced from calendar"
          }
        >
          📅 {m.due_date ? formatDateOnly(msDate(m)) : "—"}
        </span>
      ) : (
        <input
          type="date"
          value={date}
          onChange={(e) => setDate(e.target.value)}
          onBlur={persist}
          className="h-6 w-full rounded-[var(--radius-sm)] border border-border2 bg-surface px-0.5 font-mono text-[9px] text-ink3 focus:border-accent focus:outline-none"
        />
      )}
      <MilestoneDot met={met} onToggle={toggleDone} />
      <input
        value={label}
        onChange={(e) => setLabel(e.target.value)}
        onBlur={persist}
        placeholder="label"
        className={`w-full rounded-[var(--radius-sm)] bg-transparent px-0.5 text-center text-[10px] text-ink2 focus:outline-none ${
          met ? "text-ink4 line-through" : ""
        }`}
      />
      <button
        onClick={remove}
        aria-label="Remove milestone"
        className="text-[10px] text-ink4 hover:text-st-due"
      >
        ✕
      </button>
      {showPower && m.event_missing && <span className="text-[9px] text-st-due">⚠ unsynced</span>}
    </div>
  );
}

/** One milestone as a list row: done dot · date · label · remove — the same edits as the column
 *  view, laid out horizontally. */
function MilestoneRow({ m, onChanged, onError, showPower }: MilestoneItemProps) {
  const { label, setLabel, date, setDate, met, persist, toggleDone, remove } = useMilestoneEditor(
    m,
    onChanged,
    onError,
  );
  return (
    <div className="flex items-center gap-1">
      <MilestoneDot met={met} onToggle={toggleDone} />
      {m.calendar_linked ? (
        <span
          className="flex h-6 w-[6.25rem] shrink-0 items-center gap-0.5 font-mono text-[9px] text-accent-text"
          title={
            m.event_missing ? "Linked event not found in your calendars" : "Synced from calendar"
          }
        >
          📅 {m.due_date ? formatDateOnly(msDate(m)) : "—"}
        </span>
      ) : (
        <input
          type="date"
          value={date}
          onChange={(e) => setDate(e.target.value)}
          onBlur={persist}
          className="w-[6.25rem] shrink-0 rounded-[var(--radius-sm)] border border-border2 bg-surface px-1 py-0.5 font-mono text-[10px] text-ink3 focus:border-accent focus:outline-none"
        />
      )}
      <input
        value={label}
        onChange={(e) => setLabel(e.target.value)}
        onBlur={persist}
        placeholder="label"
        className={`min-w-0 flex-1 rounded-[var(--radius-sm)] border border-transparent bg-transparent px-1 py-0.5 text-xs text-ink2 focus:border-accent focus:outline-none ${
          met ? "text-ink4 line-through" : ""
        }`}
      />
      {showPower && m.event_missing && (
        <span className="shrink-0 text-[9px] text-st-due" title="Linked event not found">
          ⚠
        </span>
      )}
      <button
        onClick={remove}
        aria-label="Remove milestone"
        className="shrink-0 px-0.5 text-[11px] text-ink4 hover:text-st-due"
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
  useEffect(() => {
    listProjects()
      .then(setProjects)
      .catch(() => setProjects([]));
  }, []);
  const listId = `pm-projects-${widget.id}`;

  // Show items in date order; undated items sink to the bottom.
  const items = [...(widget.items ?? [])].sort((a, b) => {
    if (!a.date) return 1;
    if (!b.date) return -1;
    return a.date.localeCompare(b.date);
  });

  return (
    <div className="flex h-full flex-col px-2 py-1">
      {items.length === 0 ? (
        <div className="min-h-0 flex-1">
          <p className="text-[11px] text-ink4">No milestones yet.</p>
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
              <input
                type="date"
                value={it.date ?? ""}
                onChange={(e) => onUpdateItem(widget.id, it.id, { date: e.target.value })}
                title={it.date ? formatDateOnly(it.date) : "Set a date"}
                className="w-[6.25rem] shrink-0 rounded-[var(--radius-sm)] border border-border2 bg-surface px-1 py-0.5 font-mono text-[10px] text-ink3 focus:border-accent focus:outline-none"
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
                className="shrink-0 px-0.5 text-[11px] text-ink4 hover:text-st-due"
              >
                ✕
              </button>
            </div>
          ))}
        </TimelineList>
      )}
      <button
        onClick={() => onAddItem(widget.id)}
        className="mt-1 shrink-0 self-start rounded-[var(--radius-sm)] px-1 py-0.5 text-[11px] text-accent-text hover:bg-surface"
      >
        + Milestone
      </button>
      {/* Bind to a real project to sync milestones with the brief + Focus. Typing a new name is
          allowed (it's created when the first milestone is added). */}
      <div className="mt-1 flex shrink-0 items-center gap-1" data-help="pinboard-timeline-project">
        <input
          list={listId}
          value={projDraft}
          onChange={(e) => setProjDraft(e.target.value)}
          placeholder="Link a project…"
          className="min-w-0 flex-1 rounded-[var(--radius-sm)] border border-border2 bg-surface px-1 py-0.5 text-[11px] text-ink3 focus:border-accent focus:outline-none"
        />
        <datalist id={listId}>
          {projects.map((p) => (
            <option key={p} value={p} />
          ))}
        </datalist>
        <button
          onClick={() => projDraft.trim() && onChange(widget.id, { project: projDraft.trim() })}
          disabled={!projDraft.trim()}
          className="shrink-0 rounded-[var(--radius-sm)] px-1 py-0.5 text-[11px] text-accent-text hover:bg-surface disabled:opacity-40"
        >
          Link
        </button>
      </div>
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
      <input
        type="date"
        value={item.date ?? ""}
        onChange={(e) => onUpdateItem(widgetId, item.id, { date: e.target.value })}
        title={item.date ? formatDateOnly(item.date) : "Set a date"}
        className="h-6 w-full rounded-[var(--radius-sm)] border border-border2 bg-surface px-0.5 font-mono text-[9px] text-ink3 focus:border-accent focus:outline-none"
      />
      <span
        className="h-2.5 w-2.5 shrink-0 rounded-full border"
        style={{ background: "var(--accent)", borderColor: "var(--panel)" }}
      />
      <input
        value={item.label ?? ""}
        onChange={(e) => onUpdateItem(widgetId, item.id, { label: e.target.value })}
        placeholder="what happens"
        className="w-full rounded-[var(--radius-sm)] bg-transparent px-0.5 text-center text-[10px] text-ink2 focus:outline-none"
      />
      <button
        onClick={() => onRemoveItem(widgetId, item.id)}
        aria-label="Remove milestone"
        className="text-[10px] text-ink4 hover:text-st-due"
      >
        ✕
      </button>
    </div>
  );
}
