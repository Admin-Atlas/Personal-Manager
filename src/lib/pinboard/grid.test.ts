// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import { clampRect, dissolveFolders, FOLDER_MIN, minSize, resolveDrop } from "./grid";
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

describe("dissolveFolders — auto-dissolve at ≤1 child", () => {
  it("keeps a folder with ≥2 children", () => {
    const f = folder("f", { x: 2, y: 2, w: 3, h: 3 }, [
      note("a", { x: 0, y: 0, w: 7, h: 5 }),
      note("b", { x: 0, y: 0, w: 7, h: 5 }),
    ]);
    expect(dissolveFolders([f], 44, 28)).toEqual([f]);
  });

  it("replaces a one-child folder with that child at the folder's position", () => {
    const f = folder("f", { x: 2, y: 3, w: 3, h: 3 }, [note("a", { x: 9, y: 9, w: 7, h: 5 })]);
    const out = dissolveFolders([f], 44, 28);
    expect(out).toHaveLength(1);
    expect(out[0].id).toBe("a");
    expect(out[0].rect).toMatchObject({ x: 2, y: 3, w: 7, h: 5 });
  });

  it("drops an empty folder", () => {
    const f = folder("f", { x: 0, y: 0, w: 3, h: 3 }, []);
    expect(dissolveFolders([f], 44, 28)).toEqual([]);
  });
});

describe("resolveDrop — merge / add / move", () => {
  const size: Rect = { x: 4, y: 4, w: 7, h: 5 };

  it("plain-moves when nothing else is at the drop", () => {
    const widgets = [note("a", { x: 0, y: 0, w: 7, h: 5 }), note("b", { x: 20, y: 0, w: 7, h: 5 })];
    const out = resolveDrop(widgets, "a", size, 44, 28, stubId);
    expect(out.find((w) => w.id === "a")!.rect).toEqual(size);
    expect(out).toHaveLength(2);
  });

  it("folds two identically-placed widgets into a new 3×3 folder", () => {
    const widgets = [note("a", size), note("b", { x: 0, y: 0, w: 7, h: 5 })];
    const out = resolveDrop(widgets, "b", size, 44, 28, stubId); // drop b exactly onto a
    expect(out).toHaveLength(1);
    const f = out[0];
    expect(f.kind).toBe("folder");
    expect(f.id).toBe("F");
    expect(f.rect).toMatchObject({ x: 4, y: 4, w: 3, h: 3 });
    expect(f.children?.map((c) => c.id).sort()).toEqual(["a", "b"]);
  });

  it("adds a widget dropped onto (overlapping) an existing folder", () => {
    const f = folder("f", { x: 4, y: 4, w: 3, h: 3 }, [note("a", size), note("b", size)]);
    const widgets = [f, note("c", { x: 0, y: 0, w: 7, h: 5 })];
    const out = resolveDrop(widgets, "c", { x: 4, y: 4, w: 7, h: 5 }, 44, 28, stubId);
    expect(out).toHaveLength(1);
    expect(out[0].children?.map((c) => c.id)).toEqual(["a", "b", "c"]);
  });

  it("flattens folder-onto-folder (no nesting) into the target", () => {
    const target = folder("t", { x: 4, y: 4, w: 3, h: 3 }, [note("a", size)]);
    const moved = folder("m", { x: 20, y: 4, w: 3, h: 3 }, [note("b", size), note("c", size)]);
    const out = resolveDrop([target, moved], "m", { x: 4, y: 4, w: 3, h: 3 }, 44, 28, stubId);
    expect(out).toHaveLength(1);
    expect(out[0].id).toBe("t");
    expect(out[0].children?.every((c) => c.kind !== "folder")).toBe(true);
    expect(out[0].children?.map((c) => c.id)).toEqual(["a", "b", "c"]);
  });

  it("returns the list unchanged when the moving id is absent", () => {
    const widgets = [note("a", size)];
    expect(resolveDrop(widgets, "ghost", size, 44, 28, stubId)).toBe(widgets);
  });
});
