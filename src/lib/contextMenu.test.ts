// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it, vi } from "vitest";
import {
  fallbackItems,
  inEditable,
  placeMenu,
  tidy,
  wantsAppMenu,
  type MenuItem,
} from "./contextMenu";

const item = (label: string): MenuItem => ({ label, onSelect: () => {} });
const sep: MenuItem = { kind: "separator" };
const labels = (items: MenuItem[]) => items.map((i) => (i.kind === "separator" ? "—" : i.label));

function el(html: string): Element {
  const host = document.createElement("div");
  host.innerHTML = html;
  return host.querySelector("[data-t]")!;
}

describe("tidy", () => {
  it("keeps separators only between groups", () => {
    expect(labels(tidy([sep, item("a"), sep, sep, item("b"), sep]))).toEqual(["a", "—", "b"]);
    expect(tidy([sep, sep])).toEqual([]);
  });
});

describe("inEditable", () => {
  it("is true in text you type into, and false elsewhere", () => {
    expect(inEditable(el('<input data-t type="text">'))).toBe(true);
    expect(inEditable(el("<input data-t>"))).toBe(true);
    expect(inEditable(el("<textarea data-t></textarea>"))).toBe(true);
    expect(inEditable(el('<div contenteditable="true"><p><span data-t>x</span></p></div>'))).toBe(
      true,
    );
    expect(inEditable(el('<input data-t type="checkbox">'))).toBe(false);
    expect(inEditable(el("<button data-t>x</button>"))).toBe(false);
    // A widget ProseMirror draws inside its editor (a checkbox marker) is not text.
    expect(
      inEditable(
        el('<div contenteditable="true"><span contenteditable="false" data-t></span></div>'),
      ),
    ).toBe(false);
    expect(inEditable(null)).toBe(false);
  });
});

describe("wantsAppMenu", () => {
  const target = el("<p data-t>words</p>");
  it("leaves the system menu to Shift, to text fields and to a surface that took it", () => {
    expect(wantsAppMenu({ defaultPrevented: false, shiftKey: false, target })).toBe(true);
    expect(wantsAppMenu({ defaultPrevented: false, shiftKey: true, target })).toBe(false);
    expect(wantsAppMenu({ defaultPrevented: true, shiftKey: false, target })).toBe(false);
    expect(
      wantsAppMenu({ defaultPrevented: false, shiftKey: false, target: el("<input data-t>") }),
    ).toBe(false);
  });
});

describe("placeMenu", () => {
  it("opens at the pointer when it fits", () => {
    expect(placeMenu(100, 100, 150, 200, 1000, 800)).toEqual({ left: 100, top: 100 });
  });
  it("flips left and up at the window's right and bottom edges", () => {
    expect(placeMenu(950, 700, 150, 200, 1000, 800)).toEqual({ left: 800, top: 500 });
  });
  it("stays inside a window smaller than the flip needs", () => {
    expect(placeMenu(50, 50, 150, 200, 120, 120)).toEqual({ left: 4, top: 4 });
  });
});

describe("fallbackItems", () => {
  const actions = () => ({ openUrl: vi.fn(), copy: vi.fn() });
  it("offers Copy for selected text", () => {
    const a = actions();
    const items = fallbackItems(el("<p data-t>hi</p>"), "hello", a);
    expect(labels(items)).toEqual(["Copy"]);
    (items[0] as { onSelect: () => void }).onSelect();
    expect(a.copy).toHaveBeenCalledWith("hello");
  });
  it("offers a web link's Open and Copy, with Copy for a selection after them", () => {
    const a = actions();
    const items = fallbackItems(el('<a href="https://example.com/x"><b data-t>x</b></a>'), "x", a);
    expect(labels(items)).toEqual(["Open link", "Copy link", "—", "Copy"]);
    (items[0] as { onSelect: () => void }).onSelect();
    expect(a.openUrl).toHaveBeenCalledWith("https://example.com/x");
  });
  it("offers nothing for a click on nothing in particular, or for a link that isn't a web address", () => {
    expect(fallbackItems(el("<p data-t>hi</p>"), "  ", actions())).toEqual([]);
    expect(fallbackItems(el('<a data-t href="javascript:void 0">x</a>'), "", actions())).toEqual(
      [],
    );
  });
});
