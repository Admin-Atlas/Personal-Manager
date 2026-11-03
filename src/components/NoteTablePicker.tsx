// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Table button in a pinboard note's format toolbar (#1030): a size grid like the one Docs and Word
// open, where you sweep across the squares to choose columns × rows and click to insert. Pressing,
// dragging and letting go works too, and so do the arrow keys and Enter.
//
// It only chooses a size. The table itself is Markdown text, written by noteTables' insertTable
// through the caller's `onPick`.
//
// With the cursor in a table (the document editor says so) the same button opens that table's menu
// instead, the one Docs puts on a right-click: add a row or column either side, or delete the row,
// the column or the whole table.
//
// The grid escapes the note's clipping (a note card is `overflow-hidden`, and the grid is wider than
// a small one). Its panel is portalled, so NoteBody treats a press inside any popover panel as a
// press inside the note, or opening the picker would end the edit.

import { useEffect, useRef, useState, type KeyboardEvent, type PointerEvent } from "react";
import { Popover, Tooltip } from "./ui";
import {
  TABLE_ACTIONS as TABLE_MENU,
  TABLE_PICKER_COLS,
  TABLE_PICKER_ROWS,
  type TableAction,
} from "../lib/pinboard/noteTables";

const CELL_PX = 16;
const GAP_PX = 3;

interface Size {
  cols: number;
  rows: number;
}

export function TableIcon() {
  return (
    <svg viewBox="0 0 24 24" className="h-4 w-4" fill="none" stroke="currentColor" strokeWidth={2}>
      <rect x="3.5" y="4.5" width="17" height="15" rx="2" />
      <line x1="3.5" y1="10" x2="20.5" y2="10" />
      <line x1="3.5" y1="15" x2="20.5" y2="15" />
      <line x1="12" y1="4.5" x2="12" y2="19.5" />
    </svg>
  );
}

export function NoteTablePicker({
  onPick,
  buttonClassName,
  table,
}: {
  onPick: (cols: number, rows: number) => void;
  buttonClassName: string;
  /** Set while the cursor is in a table: the button opens that table's menu instead of the grid. */
  table?: { act: (action: TableAction) => void; can: (action: TableAction) => boolean };
}) {
  const [open, setOpen] = useState(false);
  return (
    <Popover
      open={open}
      onOpenChange={setOpen}
      escapeClipping
      withinDialog
      // A flex box, not Popover's plain block: in a block the button sat on a text line, and the
      // line's descender space under it lifted the icon above its neighbours in the toolbar.
      rootClassName="flex"
      ariaLabel={table ? "Table options" : "Insert a table"}
      panelClassName={table ? "min-w-0 p-1" : "min-w-0 p-2"}
      trigger={({ toggle }) => (
        <Tooltip label="Table">
          <button
            type="button"
            aria-label="Table"
            aria-expanded={open}
            // Keep the textarea's caret: the table goes where it is.
            onMouseDown={(e) => e.preventDefault()}
            onClick={toggle}
            className={buttonClassName}
          >
            <TableIcon />
          </button>
        </Tooltip>
      )}
    >
      {open && table && <TableMenu table={table} onDone={() => setOpen(false)} />}
      {open && !table && (
        <SizeGrid
          onPick={(s) => {
            setOpen(false);
            onPick(s.cols, s.rows);
          }}
        />
      )}
    </Popover>
  );
}

function TableMenu({
  table,
  onDone,
}: {
  table: NonNullable<Parameters<typeof NoteTablePicker>[0]["table"]>;
  onDone: () => void;
}) {
  const menuRef = useRef<HTMLDivElement>(null);
  // Read once on open: the cursor can't move while the menu has focus.
  const [can] = useState(() => new Map(TABLE_MENU.map((m) => [m.action, table.can(m.action)])));

  useEffect(() => {
    menuRef.current?.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus();
  }, []);

  function onKeyDown(e: KeyboardEvent) {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp") return;
    e.preventDefault();
    e.stopPropagation();
    const items = [
      ...menuRef.current!.querySelectorAll<HTMLButtonElement>("button:not(:disabled)"),
    ];
    const at = items.indexOf(document.activeElement as HTMLButtonElement);
    const step = e.key === "ArrowDown" ? 1 : -1;
    items[(at + step + items.length) % items.length]?.focus();
  }

  return (
    <div
      ref={menuRef}
      role="menu"
      aria-label="Table"
      onKeyDown={onKeyDown}
      className="flex flex-col text-[0.8125rem]"
    >
      {TABLE_MENU.map(({ action, label }) => (
        <button
          key={action}
          type="button"
          role="menuitem"
          disabled={!can.get(action)}
          onClick={() => {
            onDone();
            table.act(action);
          }}
          className={`rounded-[var(--radius-sm)] px-2 py-1 text-left hover:bg-surface focus:bg-surface focus:outline-none disabled:opacity-50 ${
            action.startsWith("delete") ? "text-st-due" : "text-ink2"
          } ${action === "deleteRow" ? "mt-1 border-t border-[var(--rule)] pt-1.5" : ""}`}
        >
          {label}
        </button>
      ))}
    </div>
  );
}

function SizeGrid({ onPick }: { onPick: (s: Size) => void }) {
  const [size, setSize] = useState<Size>({ cols: 1, rows: 1 });
  const gridRef = useRef<HTMLDivElement>(null);
  const pressed = useRef(false);

  // Focus the grid on open, so the arrow keys work straight away. The note's textarea keeps its
  // selection while blurred, and the insert puts focus back there.
  useEffect(() => {
    gridRef.current?.focus();
  }, []);

  // The square under the pointer, clamped to the grid, so a drag that runs past the edge still
  // reads as the biggest size rather than dropping back.
  function sizeAt(e: PointerEvent): Size {
    const r = gridRef.current!.getBoundingClientRect();
    const step = CELL_PX + GAP_PX;
    const clamp = (n: number, max: number) => Math.min(max, Math.max(1, n));
    return {
      cols: clamp(Math.floor((e.clientX - r.left) / step) + 1, TABLE_PICKER_COLS),
      rows: clamp(Math.floor((e.clientY - r.top) / step) + 1, TABLE_PICKER_ROWS),
    };
  }

  function onKeyDown(e: KeyboardEvent) {
    const move: Record<string, [number, number]> = {
      ArrowRight: [1, 0],
      ArrowLeft: [-1, 0],
      ArrowDown: [0, 1],
      ArrowUp: [0, -1],
    };
    const d = move[e.key];
    if (d) {
      e.preventDefault();
      e.stopPropagation();
      setSize((s) => ({
        cols: Math.min(TABLE_PICKER_COLS, Math.max(1, s.cols + d[0])),
        rows: Math.min(TABLE_PICKER_ROWS, Math.max(1, s.rows + d[1])),
      }));
    } else if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      e.stopPropagation();
      onPick(size);
    }
  }

  const label = `${size.cols} × ${size.rows}`;
  return (
    <div className="flex flex-col items-center gap-1.5">
      <div
        ref={gridRef}
        tabIndex={0}
        role="slider"
        aria-label="Table size, columns by rows"
        aria-valuetext={`${size.cols} columns, ${size.rows} rows`}
        aria-valuenow={size.cols * size.rows}
        onKeyDown={onKeyDown}
        onPointerDown={(e) => {
          pressed.current = true;
          e.currentTarget.setPointerCapture(e.pointerId);
          setSize(sizeAt(e));
        }}
        onPointerMove={(e) => setSize(sizeAt(e))}
        onPointerUp={(e) => {
          if (!pressed.current) return;
          pressed.current = false;
          onPick(sizeAt(e));
        }}
        className="grid touch-none rounded-[var(--radius-sm)] focus:outline-none focus-visible:ring-2 focus-visible:ring-[var(--accent)]"
        style={{
          gridTemplateColumns: `repeat(${TABLE_PICKER_COLS}, ${CELL_PX}px)`,
          gap: GAP_PX,
        }}
      >
        {Array.from({ length: TABLE_PICKER_ROWS }, (_, r) =>
          Array.from({ length: TABLE_PICKER_COLS }, (_, c) => {
            const on = c < size.cols && r < size.rows;
            return (
              <span
                key={`${r}-${c}`}
                aria-hidden
                className={`block rounded-[2px] border ${
                  on ? "border-[var(--accent)] bg-accent-soft" : "border-[var(--ink4)]"
                }`}
                style={{ width: CELL_PX, height: CELL_PX }}
              />
            );
          }),
        )}
      </div>
      <p className="font-mono text-[0.6875rem] text-ink3" aria-hidden>
        {label}
      </p>
    </div>
  );
}
