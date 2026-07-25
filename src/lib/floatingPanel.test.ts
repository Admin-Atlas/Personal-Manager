// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import {
  clampPanel,
  defaultPanelRect,
  movePanel,
  resizePanel,
  MIN_H,
  MIN_W,
  TITLE_BAR_H,
} from "./floatingPanel";

const VIEW = { w: 1280, h: 800 };

describe("clampPanel", () => {
  it("leaves a rect that already fits alone", () => {
    const r = { x: 100, y: 100, w: 320, h: 260 };
    expect(clampPanel(r, VIEW)).toEqual(r);
  });

  it("never lets the panel cover the title bar", () => {
    // That strip carries the window drag region and the min/max/close buttons.
    expect(clampPanel({ x: 10, y: 0, w: 320, h: 260 }, VIEW).y).toBe(TITLE_BAR_H);
    expect(clampPanel({ x: 10, y: -500, w: 320, h: 260 }, VIEW).y).toBe(TITLE_BAR_H);
  });

  it("pulls a panel back on-screen from the right and bottom", () => {
    const r = clampPanel({ x: 5000, y: 5000, w: 320, h: 260 }, VIEW);
    expect(r.x).toBe(VIEW.w - 320);
    expect(r.y).toBe(VIEW.h - 260);
  });

  it("enforces the minimum size", () => {
    const r = clampPanel({ x: 10, y: 100, w: 10, h: 10 }, VIEW);
    expect(r.w).toBe(MIN_W);
    expect(r.h).toBe(MIN_H);
  });

  it("shrinks a panel too large for the viewport", () => {
    const r = clampPanel({ x: 0, y: TITLE_BAR_H, w: 99999, h: 99999 }, VIEW);
    expect(r.w).toBeLessThanOrEqual(VIEW.w);
    expect(r.h).toBeLessThanOrEqual(VIEW.h);
  });

  it("rescues geometry saved on a bigger monitor", () => {
    // The whole point of re-clamping on load: a panel parked at x=1800 on a wide screen must not
    // be invisible when the app reopens on a laptop.
    const saved = { x: 1800, y: 900, w: 400, h: 300 };
    const r = clampPanel(saved, { w: 1024, h: 768 });
    expect(r.x).toBeGreaterThanOrEqual(0);
    expect(r.x + r.w).toBeLessThanOrEqual(1024);
    expect(r.y).toBeGreaterThanOrEqual(TITLE_BAR_H);
    expect(r.y + r.h).toBeLessThanOrEqual(768);
  });

  it("keeps the top-left reachable on a viewport smaller than the minimum size", () => {
    const r = clampPanel({ x: 500, y: 500, w: 320, h: 260 }, { w: 200, h: 120 });
    expect(r.x).toBe(0);
    expect(r.y).toBe(TITLE_BAR_H);
    // Size is not shrunk below usability, so it may overflow — but it can be dragged.
    expect(r.w).toBe(MIN_W);
    expect(r.h).toBe(MIN_H);
  });
});

describe("defaultPanelRect", () => {
  it("opens near the top-right, clear of the title bar", () => {
    const r = defaultPanelRect(VIEW);
    expect(r.y).toBeGreaterThanOrEqual(TITLE_BAR_H);
    expect(r.x + r.w).toBeLessThanOrEqual(VIEW.w);
  });

  it("is usable on a small window too", () => {
    const r = defaultPanelRect({ w: 640, h: 480 });
    expect(r.x).toBeGreaterThanOrEqual(0);
    expect(r.y).toBeGreaterThanOrEqual(TITLE_BAR_H);
  });
});

describe("movePanel / resizePanel", () => {
  const start = { x: 100, y: 100, w: 320, h: 260 };

  it("moves by the delta without changing size", () => {
    const r = movePanel(start, 50, -20, VIEW);
    expect(r).toEqual({ x: 150, y: 80, w: 320, h: 260 });
  });

  it("clamps a move that would go under the title bar", () => {
    expect(movePanel(start, 0, -1000, VIEW).y).toBe(TITLE_BAR_H);
  });

  it("resizes from the bottom-right without moving the origin", () => {
    const r = resizePanel(start, 40, 30, VIEW);
    expect(r.x).toBe(start.x);
    expect(r.y).toBe(start.y);
    expect(r.w).toBe(360);
    expect(r.h).toBe(290);
  });

  it("will not resize below the minimum", () => {
    const r = resizePanel(start, -9999, -9999, VIEW);
    expect(r.w).toBe(MIN_W);
    expect(r.h).toBe(MIN_H);
  });
});
