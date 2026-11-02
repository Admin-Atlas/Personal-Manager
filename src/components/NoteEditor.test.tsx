// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { createRef } from "react";
import { act, cleanup, fireEvent, render } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { NoteEditor, type NoteEditorHandle } from "./NoteEditor";
import { toggleWrap, type TextEdit } from "../lib/pinboard/notesMarkdown";

vi.mock("../lib/ipc", () => ({ openUrl: vi.fn(() => Promise.resolve()) }));

// jsdom lays nothing out; CodeMirror measures ranges when it draws a cursor or scrolls one into view.
beforeAll(() => {
  const rect = { left: 0, right: 0, top: 0, bottom: 0, width: 0, height: 0, x: 0, y: 0 };
  Range.prototype.getClientRects = () =>
    ({
      length: 0,
      item: () => null,
      [Symbol.iterator]: [][Symbol.iterator],
    }) as unknown as DOMRectList;
  Range.prototype.getBoundingClientRect = () => ({ ...rect, toJSON: () => rect }) as DOMRect;
});

afterEach(cleanup);

function mount(
  value: string,
  keyEdit: (...args: unknown[]) => TextEdit | null = vi.fn(() => null),
) {
  const onChange = vi.fn();
  const ref = createRef<NoteEditorHandle>();
  const utils = render(
    <NoteEditor
      ref={ref}
      value={value}
      onChange={onChange}
      onFocus={() => {}}
      keyEdit={keyEdit}
      placeholder="Jot something down…"
    />,
  );
  return { ...utils, onChange, ref };
}

describe("NoteEditor", () => {
  it("draws the note formatted, not as raw Markdown", () => {
    const { container } = mount("## Plan\n. milk\n[] call Sam\nsome **bold** text");
    expect(container.querySelector(".pm-cm-h2")).not.toBeNull();
    expect(container.querySelector(".pm-cm-bullet")?.textContent).toBe("•");
    expect(container.querySelector(".pm-cm-box")).not.toBeNull();
    expect(container.querySelector(".pm-cm-strong")?.textContent).toBe("bold");
    expect(container.textContent).not.toContain("**");
    expect(container.textContent).not.toContain("##");
  });

  it("ticks a checkbox in place, the way the rendered note did", () => {
    const { container, onChange } = mount("[] call Sam");
    fireEvent.mouseDown(container.querySelector(".pm-cm-box")!);
    expect(onChange).toHaveBeenLastCalledWith("[x] call Sam");
  });

  it("draws a table as a grid", () => {
    const { container } = mount("| a | b |\n| --- | --- |\n| 1 | 2 |");
    const table = container.querySelector(".pm-cm-table table") as HTMLTableElement;
    expect(table.rows).toHaveLength(2);
    expect(table.rows[1].cells[1].textContent).toBe("2");
  });

  it("applies a toolbar edit at the selection and reports it", () => {
    const { ref, onChange } = mount("word");
    act(() => ref.current!.apply((v) => toggleWrap(v, 0, 4, "**")));
    expect(onChange).toHaveBeenLastCalledWith("**word**");
  });

  it("takes a new value from outside without echoing it back", () => {
    const { rerender, onChange, container } = mount("one");
    rerender(
      <NoteEditor
        value="one two"
        onChange={onChange}
        onFocus={() => {}}
        keyEdit={() => null}
        placeholder=""
      />,
    );
    expect(container.querySelector(".cm-content")?.textContent).toBe("one two");
    expect(onChange).not.toHaveBeenCalled();
  });

  it("hands keypresses to keyEdit and applies what it returns", () => {
    const keyEdit = vi.fn(() => ({ text: "a\n. ", selStart: 4, selEnd: 4 }));
    const { container, onChange } = mount("a", keyEdit);
    fireEvent.keyDown(container.querySelector(".cm-content")!, { key: "Enter" });
    expect(keyEdit).toHaveBeenCalled();
    expect(onChange).toHaveBeenLastCalledWith("a\n. ");
  });
});
