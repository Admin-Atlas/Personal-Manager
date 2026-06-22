// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useState, type PointerEvent as ReactPointerEvent } from "react";
import { formatDate } from "../lib/format";
import { CELL, COLS, MIN_H, MIN_W, ROWS, pxRectToCells } from "../lib/pinboard/grid";
import { usePinboard } from "../lib/pinboard/usePinboard";
import type { Rect, Widget } from "../lib/pinboard/types";
import { useDepth } from "../theme";
import { Button, Input, Textarea } from "./ui";

/** Note tint options — design tokens, never hex, so they track the active theme. */
const NOTE_COLORS = ["st-quick", "st-due", "st-look", "st-track", "st-part"];

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
 * `lib/pinboard/grid.ts`. Depth-aware: notes at every depth, timelines from `standard` up,
 * per-widget metadata at `power`.
 */
export function PinboardView() {
  const { showMeta, showPower } = useDepth();
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
  } = usePinboard();

  const [drag, setDrag] = useState<DragStart | null>(null);
  const [livePx, setLivePx] = useState<PxRect | null>(null);

  // While a drag is active, track the pointer on the window so it keeps following even when
  // the cursor leaves the widget; commit the snapped cell rect on release. Effect re-runs only
  // when a drag starts/ends (livePx lives in its own state, off the dependency list).
  useEffect(() => {
    if (!drag) return;
    const startPx = rectToPx(drag.startRect);
    const compute = (e: PointerEvent): PxRect => {
      const dx = e.clientX - drag.startX;
      const dy = e.clientY - drag.startY;
      if (drag.mode === "move") {
        return {
          x: Math.max(0, Math.min(startPx.x + dx, COLS * CELL - startPx.w)),
          y: Math.max(0, Math.min(startPx.y + dy, ROWS * CELL - startPx.h)),
          w: startPx.w,
          h: startPx.h,
        };
      }
      return {
        x: startPx.x,
        y: startPx.y,
        w: Math.max(MIN_W * CELL, Math.min(startPx.w + dx, COLS * CELL - startPx.x)),
        h: Math.max(MIN_H * CELL, Math.min(startPx.h + dy, ROWS * CELL - startPx.y)),
      };
    };
    const onMove = (e: PointerEvent) => setLivePx(compute(e));
    const onUp = (e: PointerEvent) => {
      moveWidget(drag.id, pxRectToCells(compute(e)));
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
          <Button variant="secondary" onClick={addNote} className="px-2.5 py-1 text-xs">
            + Note
          </Button>
          {/* Timelines are a richer surface — offered from standard depth up. */}
          {showMeta && (
            <Button variant="secondary" onClick={addTimeline} className="px-2.5 py-1 text-xs">
              + Timeline
            </Button>
          )}
        </div>
      </header>

      <div className="flex-1 overflow-auto p-6">
        <div
          className={`relative mx-auto rounded-[var(--radius)] border border-border ${
            drag ? "select-none" : ""
          }`}
          style={{
            width: COLS * CELL,
            height: ROWS * CELL,
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
                    <NoteBody widget={w} onChange={updateWidget} />
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

function NoteBody({
  widget,
  onChange,
}: {
  widget: Widget;
  onChange: (id: string, patch: Partial<Widget>) => void;
}) {
  return (
    <div className="flex h-full flex-col">
      <Textarea
        value={widget.text ?? ""}
        onChange={(e) => onChange(widget.id, { text: e.target.value })}
        placeholder="Jot something down…"
        className="min-h-0 flex-1 resize-none border-0 bg-transparent text-sm leading-snug focus:ring-0"
      />
      <div className="flex shrink-0 items-center gap-1 px-2 pb-1">
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
  );
}

function TimelineBody({
  widget,
  showPower,
  onChange,
  onAddItem,
  onUpdateItem,
  onRemoveItem,
}: {
  widget: Widget;
  showPower: boolean;
  onChange: (id: string, patch: Partial<Widget>) => void;
  onAddItem: (id: string) => void;
  onUpdateItem: (id: string, itemId: string, patch: { date?: string; label?: string }) => void;
  onRemoveItem: (id: string, itemId: string) => void;
}) {
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
        {items.length === 0 && (
          <p className="text-[11px] text-ink4">No milestones yet.</p>
        )}
        {items.map((it) => (
          <div key={it.id} className="flex items-center gap-1">
            <input
              type="date"
              value={it.date ?? ""}
              onChange={(e) => onUpdateItem(widget.id, it.id, { date: e.target.value })}
              title={it.date ? formatDate(it.date) : "Set a date"}
              className="w-[7.5rem] shrink-0 rounded-[var(--radius-sm)] border border-border2 bg-surface px-1 py-0.5 font-mono text-[10px] text-ink3 focus:border-accent focus:outline-none"
            />
            <input
              value={it.label ?? ""}
              onChange={(e) => onUpdateItem(widget.id, it.id, { label: e.target.value })}
              placeholder="what happens"
              className="min-w-0 flex-1 rounded-[var(--radius-sm)] border border-transparent bg-transparent px-1 py-0.5 text-xs text-ink2 focus:border-accent focus:outline-none"
            />
            {showPower && it.date && (
              <span className="shrink-0 font-mono text-[9px] text-faint">{formatDate(it.date)}</span>
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
    </div>
  );
}
