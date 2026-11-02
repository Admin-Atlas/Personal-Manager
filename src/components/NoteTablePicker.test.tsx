// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { NoteTablePicker } from "./NoteTablePicker";

// jsdom has no ResizeObserver (Popover's escape-clipping placement makes one) and no pointer capture.
beforeAll(() => {
  if (!("ResizeObserver" in globalThis)) {
    (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
  Element.prototype.setPointerCapture ??= () => {};
});

afterEach(cleanup);

/** The grid at 0,0 with 19px steps (16px squares, 3px gaps): the centre of square (col, row). */
function at(col: number, row: number) {
  return { clientX: (col - 1) * 19 + 8, clientY: (row - 1) * 19 + 8, pointerId: 1 };
}

function openGrid() {
  const onPick = vi.fn();
  render(<NoteTablePicker onPick={onPick} buttonClassName="" />);
  fireEvent.click(screen.getByRole("button", { name: "Table" }));
  const grid = screen.getByRole("slider", { name: /table size/i });
  grid.getBoundingClientRect = () => ({ left: 0, top: 0 }) as DOMRect;
  return { grid, onPick };
}

describe("NoteTablePicker", () => {
  it("picks the size swept to, on a click", () => {
    const { grid, onPick } = openGrid();
    fireEvent.pointerMove(grid, at(3, 2));
    expect(grid.getAttribute("aria-valuetext")).toBe("3 columns, 2 rows");
    fireEvent.pointerDown(grid, at(3, 2));
    fireEvent.pointerUp(grid, at(3, 2));
    expect(onPick).toHaveBeenCalledWith(3, 2);
    // And closes.
    expect(screen.queryByRole("slider")).toBeNull();
  });

  it("picks where a press-and-drag is let go, clamped to the grid", () => {
    const { grid, onPick } = openGrid();
    fireEvent.pointerDown(grid, at(1, 1));
    fireEvent.pointerMove(grid, at(20, 4));
    fireEvent.pointerUp(grid, at(20, 4));
    expect(onPick).toHaveBeenCalledWith(8, 4);
  });

  it("works from the keyboard", () => {
    const { grid, onPick } = openGrid();
    expect(document.activeElement).toBe(grid);
    fireEvent.keyDown(grid, { key: "ArrowRight" });
    fireEvent.keyDown(grid, { key: "ArrowRight" });
    fireEvent.keyDown(grid, { key: "ArrowDown" });
    fireEvent.keyDown(grid, { key: "ArrowUp" });
    fireEvent.keyDown(grid, { key: "ArrowUp" });
    fireEvent.keyDown(grid, { key: "Enter" });
    expect(onPick).toHaveBeenCalledWith(3, 1);
  });

  it("marks its panel as a popover panel, which the note reads as inside the note", () => {
    const { grid } = openGrid();
    expect(grid.closest("[data-popover-panel]")).not.toBeNull();
  });
});
