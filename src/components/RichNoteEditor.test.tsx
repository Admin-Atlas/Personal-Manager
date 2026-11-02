// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { createRef } from "react";
import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { RichNoteEditor, type RichNoteEditorHandle } from "./RichNoteEditor";

vi.mock("../lib/ipc", () => ({ openUrl: vi.fn(() => Promise.resolve()) }));

// jsdom lays nothing out; ProseMirror measures when it scrolls a selection into view.
beforeAll(() => {
  const rect = { left: 0, right: 0, top: 0, bottom: 0, width: 0, height: 0, x: 0, y: 0 };
  const rects = () =>
    ({
      length: 0,
      item: () => null,
      [Symbol.iterator]: [][Symbol.iterator],
    }) as unknown as DOMRectList;
  Range.prototype.getClientRects = rects;
  Range.prototype.getBoundingClientRect = () => ({ ...rect, toJSON: () => rect }) as DOMRect;
  Element.prototype.getClientRects = rects;
  document.elementFromPoint = () => null;
});

afterEach(cleanup);

function mount(value: string) {
  const onChange = vi.fn();
  const ref = createRef<RichNoteEditorHandle>();
  const utils = render(
    <RichNoteEditor
      ref={ref}
      value={value}
      onChange={onChange}
      onFocus={() => {}}
      placeholder="Jot something down…"
    />,
  );
  /** Put the cursor at the end of the text `after`. */
  const caretAfter = (after: string) => {
    const pm = utils.container.querySelector(".ProseMirror")!;
    const walker = document.createTreeWalker(pm, NodeFilter.SHOW_TEXT);
    let node: Node | null;
    while ((node = walker.nextNode())) {
      const at = node.textContent!.indexOf(after);
      if (at >= 0) {
        const range = document.createRange();
        range.setStart(node, at + after.length);
        range.collapse(true);
        (pm as HTMLElement).focus();
        const sel = window.getSelection()!;
        sel.removeAllRanges();
        sel.addRange(range);
        document.dispatchEvent(new Event("selectionchange"));
        return;
      }
    }
    throw new Error(`no text ${after}`);
  };
  return { ...utils, onChange, ref, caretAfter };
}

describe("RichNoteEditor", () => {
  it("shows the note with no Markdown on screen", () => {
    const { container } = mount(
      "## Plan\n. milk\n- dash\n[] call Sam\n1. first\nsome **bold** text",
    );
    const text = container.textContent ?? "";
    expect(container.querySelector("h2")?.textContent).toBe("Plan");
    expect(container.querySelector("strong")?.textContent).toBe("bold");
    for (const marker of ["##", "**", "[]", ". milk"]) expect(text).not.toContain(marker);
    expect(container.querySelector(".pm-rich-bullet")?.textContent).toBe("•");
    expect(container.querySelector(".pm-rich-dash")?.textContent).toBe("–");
    expect(container.querySelector(".pm-rich-task")).not.toBeNull();
    expect(container.querySelector(".pm-rich-number")?.textContent).toBe("1.");
  });

  it("shows a table as a grid you type in, not as pipes", () => {
    const { container } = mount("| a | b |\n| --- | --- |\n| 1 | 2 |");
    const table = container.querySelector("table")!;
    expect(table.querySelectorAll("th")).toHaveLength(2);
    expect(table.querySelectorAll("td")[1].textContent).toBe("2");
    expect(container.textContent).not.toContain("|");
    expect(table.closest("[contenteditable=true]")).not.toBeNull();
  });

  it("ticks a checkbox and writes the note's own marker back", () => {
    const { container, onChange } = mount("[] call Sam");
    fireEvent.mouseDown(container.querySelector(".pm-rich-task")!);
    expect(onChange).toHaveBeenLastCalledWith("[x] call Sam");
  });

  it("inserts a table where the cursor is and writes it as Markdown", () => {
    const { ref, onChange, caretAfter } = mount("before");
    caretAfter("before");
    act(() => ref.current!.insertTable(2, 2));
    expect(onChange).toHaveBeenLastCalledWith(
      "before\n|     |     |\n| --- | --- |\n|     |     |",
    );
  });

  it("formats from the toolbar", () => {
    const { ref, onChange, caretAfter } = mount("milk");
    caretAfter("milk");
    act(() => ref.current!.format("checkbox"));
    expect(onChange).toHaveBeenLastCalledWith("[] milk");
    act(() => ref.current!.format("bullet"));
    expect(onChange).toHaveBeenLastCalledWith(". milk");
    act(() => ref.current!.format("bullet"));
    expect(onChange).toHaveBeenLastCalledWith("milk");
    act(() => ref.current!.format("heading"));
    expect(onChange).toHaveBeenLastCalledWith("# milk");
  });

  it("continues a list on Enter with the next marker, and leaves it from an empty item", () => {
    const { container, onChange, caretAfter } = mount("9. nine");
    const pm = container.querySelector(".ProseMirror")!;
    caretAfter("nine");
    fireEvent.keyDown(pm, { key: "Enter" });
    expect(onChange).toHaveBeenLastCalledWith("9. nine\n10. ");
    fireEvent.keyDown(pm, { key: "Enter" });
    expect(onChange).toHaveBeenLastCalledWith("9. nine\n");
  });

  it("moves through a table with Tab, adding a row from the last cell", () => {
    const { container, onChange, caretAfter } = mount("| a | b |\n| --- | --- |");
    const pm = container.querySelector(".ProseMirror")!;
    caretAfter("b");
    fireEvent.keyDown(pm, { key: "Tab" });
    expect(onChange).toHaveBeenLastCalledWith("| a   | b   |\n| --- | --- |\n|     |     |");
  });

  it("leaves a table from Enter on an empty last row, dropping that row", () => {
    const { container, onChange, caretAfter } = mount("| a | b |\n| --- | --- |\n| 1 | 2 |");
    const pm = container.querySelector(".ProseMirror")!;
    caretAfter("1");
    fireEvent.keyDown(pm, { key: "Enter" });
    expect(onChange).toHaveBeenLastCalledWith(
      "| a   | b   |\n| --- | --- |\n| 1   | 2   |\n|     |     |",
    );
    fireEvent.keyDown(pm, { key: "Enter" });
    expect(onChange).toHaveBeenLastCalledWith("| a   | b   |\n| --- | --- |\n| 1   | 2   |\n");
    // The cursor is under the table now: typing goes there.
    fireEvent.keyDown(pm, { key: "Enter" });
    expect(onChange).toHaveBeenLastCalledWith("| a   | b   |\n| --- | --- |\n| 1   | 2   |\n\n");
  });

  it("deletes the table the cursor is in, and its rows and columns", () => {
    const { ref, onChange, caretAfter } = mount(
      "before\n| a | b |\n| --- | --- |\n| 1 | 2 |\nafter",
    );
    caretAfter("2");
    act(() => ref.current!.table("colLeft"));
    expect(onChange).toHaveBeenLastCalledWith(
      "before\n| a   |     | b   |\n| --- | --- | --- |\n| 1   |     | 2   |\nafter",
    );
    act(() => ref.current!.table("deleteCol"));
    act(() => ref.current!.table("deleteCol"));
    expect(onChange).toHaveBeenLastCalledWith("before\n| a   |\n| --- |\n| 1   |\nafter");
    // The table's only column goes only with the table.
    expect(ref.current!.canTable("deleteCol")).toBe(false);
    act(() => ref.current!.table("deleteTable"));
    expect(onChange).toHaveBeenLastCalledWith("before\nafter");
    expect(ref.current!.canTable("deleteTable")).toBe(false);
  });

  it("deletes a table from two Backspaces just under it: the first selects every cell", () => {
    const { container, onChange } = mount("| a |\n| --- |\nafter");
    const pm = container.querySelector(".ProseMirror") as HTMLElement;
    const node = [...pm.querySelectorAll("p")].find((p) => p.textContent === "after")!;
    const range = document.createRange();
    range.setStart(node.firstChild!, 0);
    pm.focus();
    window.getSelection()!.removeAllRanges();
    window.getSelection()!.addRange(range);
    document.dispatchEvent(new Event("selectionchange"));
    fireEvent.keyDown(pm, { key: "Backspace" });
    expect(onChange).not.toHaveBeenCalled();
    // prosemirror-tables turns the selected table into a selection of all of its cells.
    expect(pm.querySelectorAll(".selectedCell")).toHaveLength(1);
    fireEvent.keyDown(pm, { key: "Backspace" });
    expect(onChange).toHaveBeenLastCalledWith("after");
  });

  it("keeps the first row the header after a row goes in above it", () => {
    const { ref, container, onChange, caretAfter } = mount("| a |\n| --- |\n| 1 |");
    caretAfter("a");
    act(() => ref.current!.table("rowAbove"));
    expect(onChange).toHaveBeenLastCalledWith("|     |\n| --- |\n| a   |\n| 1   |");
    const rows = container.querySelectorAll("tr");
    expect(rows[0].querySelector("th")).not.toBeNull();
    expect(rows[1].querySelector("th")).toBeNull();
  });

  it("tells the toolbar when the cursor is in a table", () => {
    const onInTable = vi.fn();
    const ref = createRef<RichNoteEditorHandle>();
    const { container } = render(
      <RichNoteEditor
        ref={ref}
        value={"x\n| a |\n| --- |"}
        onChange={() => {}}
        onFocus={() => {}}
        onInTable={onInTable}
        placeholder=""
      />,
    );
    const pm = container.querySelector(".ProseMirror") as HTMLElement;
    const into = (text: string) => {
      const node = [...pm.querySelectorAll("p, th")].find((n) => n.textContent === text)!;
      const range = document.createRange();
      range.setStart(node.firstChild!, 1);
      pm.focus();
      window.getSelection()!.removeAllRanges();
      window.getSelection()!.addRange(range);
      document.dispatchEvent(new Event("selectionchange"));
    };
    into("a");
    expect(onInTable).toHaveBeenLastCalledWith(true);
    into("x");
    expect(onInTable).toHaveBeenLastCalledWith(false);
  });

  it("takes a new value from outside without echoing it back", () => {
    const { rerender, container, onChange } = mount("one");
    rerender(
      <RichNoteEditor value="**two**" onChange={onChange} onFocus={() => {}} placeholder="" />,
    );
    expect(container.querySelector("strong")?.textContent).toBe("two");
    expect(onChange).not.toHaveBeenCalled();
  });
});
