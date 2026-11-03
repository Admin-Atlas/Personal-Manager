// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { measureSpace } from "./measure";
import { CELL } from "./grid";

function scroller(width: number, height: number, padding: string): HTMLElement {
  const el = document.createElement("div");
  el.style.padding = padding;
  el.getBoundingClientRect = () => new DOMRect(0, 0, width, height);
  return el;
}

describe("measureSpace", () => {
  it("takes the padding as rendered, so a larger text size can't make the board overflow", () => {
    // A 960px scroller at Large text, where p-6 is 27.6px a side: 904.8px inside, so 37 cells. The
    // old fixed 48px made it 38, which is 967.2px with the padding: 7px too wide, so a scrollbar.
    const large = measureSpace(scroller(960, 600, "27.6px"));
    expect(large.cols).toBe(Math.floor((960 - 55.2) / CELL));
    expect(large.cols * CELL + 55.2).toBeLessThanOrEqual(960);
    expect(large.rows * CELL + 55.2).toBeLessThanOrEqual(600);
  });

  it("leaves room for a scrollbar on the other axis", () => {
    const s = measureSpace(scroller(1000, 600, "24px"));
    expect(s).toEqual({ cols: 39, rows: 23, barCols: 39, barRows: 22 });
  });
});
