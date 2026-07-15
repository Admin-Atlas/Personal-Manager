// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import {
  clampRect,
  FOLDER_MIN,
  folderAtPointer,
  minSize,
  rectContains,
  reflowToWidth,
  resolveDrop,
} from "./grid";
import type { Rect, Widget } from "./types";

// Deterministic id stub so resolveDrop's new-folder id is assertable.
const stubId = () => "F";

const note = (id: string, rect: Rect, extra: Partial<Widget> = {}): Widget => ({
  id,
  kind: "note",
  rect,
  ...extra,
});
const folder = (id: string, rect: Rect, children: Widget[]): Widget => ({
  id,
  kind: "folder",
  rect,
  children,
});

describe("clampRect — per-kind minimum size", () => {
  it("floors notes/timelines at 4×3 (default min)", () => {
    expect(clampRect({ x: 0, y: 0, w: 1, h: 1 }, 44, 28)).toMatchObject({ w: 4, h: 3 });
  });

  it("lets a folder tile stay 3×3 when the folder min is passed", () => {
    const r = clampRect({ x: 5, y: 5, w: 3, h: 3 }, 44, 28, FOLDER_MIN);
    expect(r).toEqual({ x: 5, y: 5, w: 3, h: 3 });
  });

  it("minSize picks the folder floor by kind", () => {
    expect(minSize("folder")).toEqual({ w: 3, h: 3 });
    expect(minSize("note")).toEqual({ w: 4, h: 3 });
  });
});

describe("reflowToWidth — fixed-width board wraps overflow to new rows", () => {
  it("leaves widgets that already fit the width exactly where they are", () => {
    const ws = [note("a", { x: 0, y: 0, w: 4, h: 3 }), note("b", { x: 4, y: 0, w: 4, h: 3 })];
    expect(reflowToWidth(ws, 10, 20)).toEqual(ws);
  });

  it("re-flows a widget that overhangs the right edge into a free slot on the same row", () => {
    const out = reflowToWidth(
      [note("a", { x: 0, y: 0, w: 4, h: 3 }), note("b", { x: 8, y: 0, w: 4, h: 3 })],
      10,
      20,
    );
    expect(out[0].rect).toEqual({ x: 0, y: 0, w: 4, h: 3 }); // fitter untouched
    expect(out[1].rect).toEqual({ x: 4, y: 0, w: 4, h: 3 }); // pulled in beside it
  });

  it("wraps an overflowing widget onto a new row when the first row is full", () => {
    const out = reflowToWidth(
      [
        note("a", { x: 0, y: 0, w: 4, h: 3 }),
        note("b", { x: 4, y: 0, w: 4, h: 3 }),
        note("c", { x: 10, y: 0, w: 4, h: 3 }), // overhangs width 8 → must wrap down
      ],
      8,
      20,
    );
    expect(out[2].rect).toEqual({ x: 0, y: 3, w: 4, h: 3 });
  });
});

describe("rectContains — a cell point inside a rect", () => {
  const r: Rect = { x: 4, y: 4, w: 3, h: 3 };

  it("contains its own origin", () => {
    expect(rectContains(r, { x: 4, y: 4 })).toBe(true);
  });

  it("contains an interior cell", () => {
    expect(rectContains(r, { x: 5, y: 6 })).toBe(true);
  });

  it("is half-open: the far edge belongs to the neighbour", () => {
    expect(rectContains(r, { x: 6, y: 6 })).toBe(true); // last cell inside
    expect(rectContains(r, { x: 7, y: 6 })).toBe(false); // x + w
    expect(rectContains(r, { x: 6, y: 7 })).toBe(false); // y + h
  });

  it("excludes cells before the origin", () => {
    expect(rectContains(r, { x: 3, y: 4 })).toBe(false);
    expect(rectContains(r, { x: 4, y: 3 })).toBe(false);
  });
});

describe("folderAtPointer — the pointer decides, not the dragged rect", () => {
  const size: Rect = { x: 0, y: 0, w: 7, h: 5 };

  it("finds the folder the pointer is inside", () => {
    const f = folder("f", { x: 4, y: 4, w: 3, h: 3 }, [note("a", size)]);
    const ws = [f, note("c", size)];
    expect(folderAtPointer(ws, "c", { x: 5, y: 5 })?.id).toBe("f");
  });

  it("finds nothing when the pointer is outside every folder", () => {
    const f = folder("f", { x: 4, y: 4, w: 3, h: 3 }, [note("a", size)]);
    expect(folderAtPointer([f, note("c", size)], "c", { x: 20, y: 20 })).toBeUndefined();
  });

  it("finds nothing without a pointer — an unknown pointer must never file", () => {
    const f = folder("f", { x: 4, y: 4, w: 3, h: 3 }, [note("a", size)]);
    expect(folderAtPointer([f, note("c", size)], "c", null)).toBeUndefined();
  });

  it("never targets a folder for a moving FOLDER — folders don't nest", () => {
    const target = folder("t", { x: 4, y: 4, w: 3, h: 3 }, [note("a", size)]);
    const moved = folder("m", { x: 4, y: 4, w: 3, h: 3 }, [note("b", size)]);
    expect(folderAtPointer([target, moved], "m", { x: 5, y: 5 })).toBeUndefined();
  });

  it("ignores notes and timelines under the pointer", () => {
    const ws = [note("a", { x: 4, y: 4, w: 7, h: 5 }), note("c", { x: 20, y: 0, w: 7, h: 5 })];
    expect(folderAtPointer(ws, "c", { x: 5, y: 5 })).toBeUndefined();
  });

  it("picks the TOP-most folder when two stack (array order is paint order)", () => {
    const under = folder("under", { x: 4, y: 4, w: 3, h: 3 }, [note("a", size)]);
    const over = folder("over", { x: 4, y: 4, w: 3, h: 3 }, [note("b", size)]);
    // `over` is later in the array ⇒ painted on top ⇒ it is the one the user sees and aims at.
    expect(folderAtPointer([under, over, note("c", size)], "c", { x: 5, y: 5 })?.id).toBe("over");
  });
});

describe("resolveDrop — file / fold / move", () => {
  const size: Rect = { x: 4, y: 4, w: 7, h: 5 };

  it("plain-moves when nothing else is at the drop", () => {
    const widgets = [note("a", { x: 0, y: 0, w: 7, h: 5 }), note("b", { x: 20, y: 0, w: 7, h: 5 })];
    const out = resolveDrop(widgets, "a", size, 44, 28, stubId, null);
    expect(out.find((w) => w.id === "a")!.rect).toEqual(size);
    expect(out).toHaveLength(2);
  });

  it("returns the SAME array when the landing is unchanged (a click without a move)", () => {
    const widgets = [note("a", size), note("b", { x: 20, y: 0, w: 7, h: 5 })];
    expect(resolveDrop(widgets, "a", size, 44, 28, stubId, null)).toBe(widgets);
  });

  it("folds two identically-placed widgets into a new 3×3 folder", () => {
    const widgets = [note("a", size), note("b", { x: 0, y: 0, w: 7, h: 5 })];
    const out = resolveDrop(widgets, "b", size, 44, 28, stubId, null); // drop b exactly onto a
    expect(out).toHaveLength(1);
    const f = out[0];
    expect(f.kind).toBe("folder");
    expect(f.id).toBe("F");
    expect(f.rect).toMatchObject({ x: 4, y: 4, w: 3, h: 3 });
    expect(f.children?.map((c) => c.id).sort()).toEqual(["a", "b"]);
  });

  it("files a widget into the folder under the POINTER", () => {
    const f = folder("f", { x: 4, y: 4, w: 3, h: 3 }, [note("a", size), note("b", size)]);
    const widgets = [f, note("c", { x: 0, y: 0, w: 7, h: 5 })];
    const out = resolveDrop(widgets, "c", { x: 4, y: 4, w: 7, h: 5 }, 44, 28, stubId, {
      x: 5,
      y: 5,
    });
    expect(out).toHaveLength(1);
    expect(out[0].children?.map((c) => c.id)).toEqual(["a", "b", "c"]);
  });

  it("does NOT file when the rect merely OVERLAPS the folder and the pointer is elsewhere", () => {
    // The old rule filed on any overlap, so a big note grazing a folder was swallowed by it.
    const f = folder("f", { x: 4, y: 4, w: 3, h: 3 }, [note("a", size), note("b", size)]);
    const drop: Rect = { x: 6, y: 4, w: 7, h: 5 }; // overlaps the folder's last column
    const widgets = [f, note("c", { x: 0, y: 0, w: 7, h: 5 })];
    const out = resolveDrop(widgets, "c", drop, 44, 28, stubId, { x: 11, y: 6 }); // pointer clear of it
    expect(out).toHaveLength(2);
    expect(out.find((w) => w.id === "f")!.children?.map((c) => c.id)).toEqual(["a", "b"]);
    expect(out.find((w) => w.id === "c")!.rect).toEqual(drop);
  });

  it("stacks folder-onto-folder instead of merging (no nesting, no notes moved)", () => {
    const target = folder("t", { x: 4, y: 4, w: 3, h: 3 }, [note("a", size)]);
    const moved = folder("m", { x: 20, y: 4, w: 3, h: 3 }, [note("b", size), note("c", size)]);
    const out = resolveDrop([target, moved], "m", { x: 4, y: 5, w: 3, h: 3 }, 44, 28, stubId, {
      x: 5,
      y: 5,
    });
    expect(out).toHaveLength(2); // both folders survive
    expect(out.find((w) => w.id === "t")!.children?.map((c) => c.id)).toEqual(["a"]);
    expect(out.find((w) => w.id === "m")!.children?.map((c) => c.id)).toEqual(["b", "c"]);
    expect(out.find((w) => w.id === "m")!.rect).toEqual({ x: 4, y: 5, w: 3, h: 3 });
  });

  it("nudges a folder dropped EXACTLY on another so the one underneath stays reachable", () => {
    const target = folder("t", { x: 4, y: 4, w: 3, h: 3 }, [note("a", size)]);
    const moved = folder("m", { x: 20, y: 4, w: 3, h: 3 }, [note("b", size)]);
    const out = resolveDrop([target, moved], "m", { x: 4, y: 4, w: 3, h: 3 }, 44, 28, stubId, {
      x: 5,
      y: 5,
    });
    expect(out).toHaveLength(2);
    expect(out.find((w) => w.id === "t")!.rect).toEqual({ x: 4, y: 4, w: 3, h: 3 });
    expect(out.find((w) => w.id === "m")!.rect).toEqual({ x: 5, y: 5, w: 3, h: 3 });
  });

  it("never folds a NOTE with a same-rect folder into a new folder", () => {
    // Folders are resizable, so a folder and a note can share a rect exactly. Folding them would
    // destroy the folder shell and swallow its notes — the twin rule is for loose cards only.
    const f = folder("f", size, [note("a", { x: 0, y: 0, w: 7, h: 5 })]);
    const widgets = [f, note("b", { x: 0, y: 0, w: 7, h: 5 })];
    const out = resolveDrop(widgets, "b", size, 44, 28, stubId, null); // pointer not over it
    expect(out).toHaveLength(2);
    expect(out.find((w) => w.id === "f")!.children?.map((c) => c.id)).toEqual(["a"]);
    expect(out.find((w) => w.id === "b")!.rect).toEqual(size);
  });

  it("never folds a moving FOLDER with a same-rect note", () => {
    const widgets = [note("a", size), folder("m", { x: 20, y: 0, w: 7, h: 5 }, [note("b", size)])];
    const out = resolveDrop(widgets, "m", size, 44, 28, stubId, null);
    expect(out).toHaveLength(2);
    expect(out.find((w) => w.id === "m")!.kind).toBe("folder");
    expect(out.find((w) => w.id === "m")!.rect).toEqual(size);
  });

  it("returns the list unchanged when the moving id is absent", () => {
    const widgets = [note("a", size)];
    expect(resolveDrop(widgets, "ghost", size, 44, 28, stubId, null)).toBe(widgets);
  });
});
