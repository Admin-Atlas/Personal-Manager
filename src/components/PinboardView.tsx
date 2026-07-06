// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import {
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
} from "react";
import { formatDateOnly } from "../lib/format";
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
import { CELL, COLS, MIN_H, MIN_W, ROWS, boundsForPx, pxRectToCells } from "../lib/pinboard/grid";
import {
  applyLineMarker,
  continueList,
  toRenderMarkdown,
  toggleWrap,
  type TextEdit,
} from "../lib/pinboard/notesMarkdown";
import { usePinboard } from "../lib/pinboard/usePinboard";
import type { Rect, Widget } from "../lib/pinboard/types";
import type { Milestone } from "../lib/types";
import { useDepth } from "../theme";
import { Button, Input, Textarea } from "./ui";

/** Note tint options — design tokens, never hex, so they track the active theme. */
const NOTE_COLORS = ["st-quick", "st-due", "st-look", "st-track", "st-part"];

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
  mode: "move" | "resize";
  startX: number;
  startY: number;
  startRect: Rect;
}

function rectToPx(r: Rect): PxRect {
  return { x: r.x * CELL, y: r.y * CELL, w: r.w * CELL, h: r.h * CELL };
}

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
  // The board's persistence/placement extent = the device screen (floored at the legacy board), so
  // a widget dragged anywhere on the enlarged board is kept on reload rather than snapped back.
  // Screen size is fixed per machine → compute once.
  const maxBounds = useMemo(
    () => boundsForPx({ w: window.screen.availWidth, h: window.screen.availHeight }),
    [],
  );
  const {
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
  } = usePinboard(maxBounds);

  const [drag, setDrag] = useState<DragStart | null>(null);
  const [livePx, setLivePx] = useState<PxRect | null>(null);

  // The board fills the window and grows to any larger window ever seen — a high-water mark of the
  // window's OWN content area. So it fits exactly when maximised (no scrollbars), and once the window
  // is made smaller than that mark it overflows and scrolls to its edges (see the `pm-scrollbars`
  // ribbons + the global wheel normaliser). Cell size and fonts are untouched — the board only gains
  // cells. Floored at the legacy 44×28. (A far-dragged widget stays reachable via boardBounds below.)
  const scrollRef = useRef<HTMLDivElement>(null);
  const [viewBounds, setViewBounds] = useState({ cols: COLS, rows: ROWS });
  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    const PAD = 24; // matches the scroll container's p-6, so board + padding never forces a scrollbar at full size
    const measure = () => {
      const availW = el.clientWidth - PAD * 2;
      const availH = el.clientHeight - PAD * 2;
      setViewBounds((prev) => ({
        cols: Math.max(prev.cols, COLS, Math.floor(availW / CELL)),
        rows: Math.max(prev.rows, ROWS, Math.floor(availH / CELL)),
      }));
    };
    measure();
    const ro = new ResizeObserver(measure);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // The board must also contain every widget, so one placed on a bigger screen stays reachable when
  // the board is reopened in a smaller window — take the max of the fill size and the widgets' extent.
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
      return {
        x: startPx.x,
        y: startPx.y,
        w: Math.max(MIN_W * CELL, Math.min(startPx.w + dx, maxX - startPx.x)),
        h: Math.max(MIN_H * CELL, Math.min(startPx.h + dy, maxY - startPx.y)),
      };
    };
    const onMove = (e: PointerEvent) => setLivePx(compute(e));
    const onUp = (e: PointerEvent) => {
      moveWidget(
        drag.id,
        pxRectToCells(compute(e), boardBoundsRef.current.cols, boardBoundsRef.current.rows),
      );
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
  }, [drag, moveWidget]);

  function startDrag(e: ReactPointerEvent, w: Widget, mode: "move" | "resize") {
    e.preventDefault();
    raiseWidget(w.id);
    setDrag({ id: w.id, mode, startX: e.clientX, startY: e.clientY, startRect: w.rect });
    setLivePx(rectToPx(w.rect));
  }

  return (
    <div className="flex h-full flex-1 flex-col">
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
            onClick={addNote}
            className="px-2.5 py-1 text-xs"
            data-help="pinboard-add-note"
          >
            + Note
          </Button>
          {/* Notes and timelines are both available at every density. */}
          <Button
            variant="secondary"
            onClick={addTimeline}
            className="px-2.5 py-1 text-xs"
            data-help="pinboard-add-timeline"
          >
            + Timeline
          </Button>
        </div>
      </header>

      <div ref={scrollRef} className="pm-scrollbars flex-1 overflow-auto p-6">
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
            const tint = w.color
              ? {
                  background: `color-mix(in oklab, var(--${w.color}) 14%, var(--panel))`,
                  borderColor: `color-mix(in oklab, var(--${w.color}) 35%, var(--border))`,
                }
              : { background: "var(--panel)", borderColor: "var(--border)" };
            return (
              <div
                key={w.id}
                data-help={w.kind === "note" ? "pinboard-note" : "pinboard-timeline"}
                className="absolute flex flex-col overflow-hidden rounded-[var(--radius-sm)] border shadow-sm transition-shadow hover:shadow-md motion-reduce:transition-none"
                style={{ left: px.x, top: px.y, width: px.w, height: px.h, ...tint }}
              >
                {/* Drag handle / header */}
                <div
                  onPointerDown={(e) => startDrag(e, w, "move")}
                  className="flex shrink-0 cursor-grab touch-none items-center justify-between gap-1 border-b border-rule px-2 py-1 active:cursor-grabbing"
                >
                  <span className="truncate font-mono text-[10px] uppercase tracking-wide text-ink4">
                    {w.kind === "note" ? "Note" : "Timeline"}
                  </span>
                  <button
                    onPointerDown={(e) => e.stopPropagation()}
                    onClick={() => removeWidget(w.id)}
                    aria-label="Delete widget"
                    title="Delete"
                    className="shrink-0 rounded-[var(--radius-sm)] px-1 text-xs text-ink4 hover:bg-surface hover:text-st-due"
                  >
                    ✕
                  </button>
                </div>

                {/* Body */}
                <div className="min-h-0 flex-1 overflow-auto">
                  {w.kind === "note" ? (
                    <NoteBody
                      widget={w}
                      onChange={updateWidget}
                      status={docStatus.get(`note:${w.id}`)}
                      onIngested={refreshDocs}
                    />
                  ) : (
                    <TimelineBody
                      widget={w}
                      showPower={showPower}
                      onChange={updateWidget}
                      onAddItem={addTimelineItem}
                      onUpdateItem={updateTimelineItem}
                      onRemoveItem={removeTimelineItem}
                    />
                  )}
                </div>

                {showPower && (
                  <div className="shrink-0 border-t border-rule px-2 py-0.5 font-mono text-[9px] text-faint">
                    {w.rect.x},{w.rect.y} · {w.rect.w}×{w.rect.h}
                  </div>
                )}

                {/* Resize handle (bottom-right) */}
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

function NoteBody({
  widget,
  onChange,
  status,
  onIngested,
}: {
  widget: Widget;
  onChange: (id: string, patch: Partial<Widget>) => void;
  status?: DocStatus;
  onIngested: () => void;
}) {
  const text = widget.text ?? "";
  const taRef = useRef<HTMLTextAreaElement>(null);
  // Render-on-idle: a filled note shows rendered Markdown (so lists read as lists); click it — or an
  // empty note — to drop into the textarea, and it re-renders on blur. No manual preview/edit toggle.
  const [editing, setEditing] = useState(false);
  const showEditor = editing || !text.trim();

  // Focus the textarea only when the user actively opens a rendered note for editing — not on load,
  // so existing empty notes don't fight over focus.
  useLayoutEffect(() => {
    if (editing) taRef.current?.focus();
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
      await ingestNote(widget.id, text);
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

  return (
    <div className="flex h-full flex-col">
      <div className="flex shrink-0 items-center gap-1 px-2 pt-1" data-help="pinboard-note-ingest">
        {!ingested ? (
          <button
            type="button"
            onClick={ingest}
            disabled={ingesting || !text.trim()}
            className="rounded-[var(--radius-sm)] px-1 text-[10px] uppercase tracking-wide text-accent-text hover:bg-surface disabled:opacity-40"
            title="Save this note to your vault as a document (it goes through Review)"
          >
            {ingesting ? "Saving…" : "Ingest"}
          </button>
        ) : (
          <>
            <span
              className="truncate text-[10px] text-ink4"
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
                onClick={ingest}
                disabled={ingesting}
                className="shrink-0 rounded-[var(--radius-sm)] px-1 text-[10px] uppercase tracking-wide text-accent-text hover:bg-surface disabled:opacity-40"
                title="Update the saved document with your latest edits"
              >
                {ingesting ? "…" : "Re-ingest"}
              </button>
            )}
          </>
        )}
      </div>
      {ingestErr && <p className="shrink-0 px-2 text-[10px] text-st-due">{ingestErr}</p>}
      {showEditor ? (
        <Textarea
          ref={taRef}
          value={text}
          onChange={(e) => onChange(widget.id, { text: e.target.value })}
          onKeyDown={onKeyDown}
          onFocus={() => setEditing(true)}
          onBlur={() => setEditing(false)}
          placeholder="Jot something down…"
          className="min-h-0 flex-1 resize-none border-0 bg-transparent text-sm leading-snug focus:ring-0"
        />
      ) : (
        <div
          className="min-h-0 flex-1 cursor-text overflow-auto px-2 text-sm"
          onClick={() => setEditing(true)}
          title="Click to edit"
        >
          <Markdown>{toRenderMarkdown(text)}</Markdown>
        </div>
      )}
      <div className="flex shrink-0 flex-wrap items-center justify-between gap-x-2 gap-y-1 px-2 pb-1">
        {showEditor && (
          <div className="flex items-center gap-0.5" data-help="pinboard-note-format">
            {FORMAT_ACTIONS.map((a) => (
              <button
                key={a.key}
                type="button"
                title={`${a.label}  (${a.hint})`}
                aria-label={`${a.label} (${a.hint})`}
                // Keep the textarea focused/selected so the edit lands where the caret is.
                onMouseDown={(e) => e.preventDefault()}
                onClick={() => applyEdit(a.apply)}
                className="flex h-5 w-5 items-center justify-center rounded-[var(--radius-sm)] text-ink4 hover:bg-surface hover:text-ink2"
              >
                {a.icon}
              </button>
            ))}
          </div>
        )}
        <div className="flex items-center gap-1" data-help="pinboard-note-tint">
          {NOTE_COLORS.map((c) => (
            <button
              key={c}
              onClick={() => onChange(widget.id, { color: c })}
              aria-label={`Tint ${c.replace("st-", "")}`}
              className={`h-3 w-3 rounded-full border ${
                widget.color === c ? "ring-1 ring-ink3" : ""
              }`}
              style={{
                background: `var(--${c})`,
                borderColor: "color-mix(in oklab, var(--ink) 20%, transparent)",
              }}
            />
          ))}
        </div>
      </div>
    </div>
  );
}

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
  onAddItem: (id: string) => void;
  onUpdateItem: (id: string, itemId: string, patch: { date?: string; label?: string }) => void;
  onRemoveItem: (id: string, itemId: string) => void;
}

/** A timeline is either *bound* to a real project — showing and editing that project's live
 *  milestones, which flow to the brief + Focus — or a freeform scratch list (the default). */
function TimelineBody(props: TimelineBodyProps) {
  if (props.widget.project) {
    return (
      <BoundTimeline
        project={props.widget.project}
        showPower={props.showPower}
        onUnlink={() => props.onChange(props.widget.id, { project: undefined })}
      />
    );
  }
  return <FreeformTimeline {...props} />;
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
  showPower,
  onUnlink,
}: {
  project: string;
  showPower: boolean;
  onUnlink: () => void;
}) {
  const [milestones, setMilestones] = useState<Milestone[]>([]);
  const [loading, setLoading] = useState(true);

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
    await addMilestone(project, "deadline", null, null);
    refresh();
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
      ) : (
        <div className="relative min-h-0 flex-1 overflow-x-auto" data-help="pinboard-timeline-line">
          {/* the line the dots sit on */}
          <div className="pointer-events-none absolute inset-x-2 top-8 h-px bg-border2" />
          <div className="flex items-start gap-1 pb-1">
            {ordered.map((m) => (
              <MilestoneColumn key={m.id} m={m} onChanged={refresh} showPower={showPower} />
            ))}
          </div>
        </div>
      )}

      <button
        onClick={add}
        className="mt-1 shrink-0 self-start rounded-[var(--radius-sm)] px-1 py-0.5 text-[11px] text-accent-text hover:bg-surface"
      >
        + Milestone
      </button>
    </div>
  );
}

/** One milestone as a column on the bound timeline: date on top, a dot on the line, its label
 *  below, and a remove ✕ — every edit persists through the milestone commands. Calendar-linked
 *  milestones show their synced date read-only. */
function MilestoneColumn({
  m,
  onChanged,
  showPower,
}: {
  m: Milestone;
  onChanged: () => void;
  showPower: boolean;
}) {
  const [label, setLabel] = useState(m.label);
  const [date, setDate] = useState(msDate(m));
  const met = m.state === "met";

  // Adopt fresh values when a refetch brings them in (e.g. a calendar-linked date syncing).
  useEffect(() => setLabel(m.label), [m.label]);
  useEffect(() => setDate(m.due_date?.slice(0, 10) ?? ""), [m.due_date]);

  async function persist() {
    const nextLabel = label.trim() || "deadline";
    const nextDate = m.calendar_linked ? null : date || null;
    const curDate = m.calendar_linked ? null : msDate(m) || null;
    if (nextLabel === m.label && nextDate === curDate) return;
    await updateMilestone(m.id, nextLabel, nextDate);
    onChanged();
  }

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
      <button
        onClick={async () => {
          await setMilestoneState(m.id, !met);
          onChanged();
        }}
        title={met ? "Mark not done" : "Mark done"}
        aria-label={met ? "Mark not done" : "Mark done"}
        className="h-2.5 w-2.5 shrink-0 rounded-full border"
        style={{
          background: met ? "var(--st-track)" : "var(--accent)",
          borderColor: "var(--panel)",
        }}
      />
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
        onClick={async () => {
          await deleteMilestone(m.id);
          onChanged();
        }}
        aria-label="Remove milestone"
        className="text-[10px] text-ink4 hover:text-st-due"
      >
        ✕
      </button>
      {showPower && m.event_missing && <span className="text-[9px] text-st-due">⚠ unsynced</span>}
    </div>
  );
}

/** The default freeform timeline: type-in dated rows that live in the widget, plus a picker to
 *  bind the card to a real project (which switches it to {@link BoundTimeline}). */
function FreeformTimeline({
  widget,
  showPower,
  onChange,
  onAddItem,
  onUpdateItem,
  onRemoveItem,
}: TimelineBodyProps) {
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
      <Input
        value={widget.title ?? ""}
        onChange={(e) => onChange(widget.id, { title: e.target.value })}
        placeholder="Timeline title"
        className="mb-1 border-0 bg-transparent px-0 text-sm font-medium focus:ring-0"
      />
      <div className="min-h-0 flex-1 space-y-1 overflow-auto">
        {items.length === 0 && <p className="text-[11px] text-ink4">No milestones yet.</p>}
        {items.map((it) => (
          <div key={it.id} className="flex items-center gap-1">
            <input
              type="date"
              value={it.date ?? ""}
              onChange={(e) => onUpdateItem(widget.id, it.id, { date: e.target.value })}
              title={it.date ? formatDateOnly(it.date) : "Set a date"}
              className="w-[7.5rem] shrink-0 rounded-[var(--radius-sm)] border border-border2 bg-surface px-1 py-0.5 font-mono text-[10px] text-ink3 focus:border-accent focus:outline-none"
            />
            <input
              value={it.label ?? ""}
              onChange={(e) => onUpdateItem(widget.id, it.id, { label: e.target.value })}
              placeholder="what happens"
              className="min-w-0 flex-1 rounded-[var(--radius-sm)] border border-transparent bg-transparent px-1 py-0.5 text-xs text-ink2 focus:border-accent focus:outline-none"
            />
            {showPower && it.date && (
              <span className="shrink-0 font-mono text-[9px] text-faint">
                {formatDateOnly(it.date)}
              </span>
            )}
            <button
              onClick={() => onRemoveItem(widget.id, it.id)}
              aria-label="Remove milestone"
              className="shrink-0 px-0.5 text-[11px] text-ink4 hover:text-st-due"
            >
              ✕
            </button>
          </div>
        ))}
      </div>
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
