// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import { keepsNativeUndo, placeConvertedTaskList, taskListDeleteCost } from "./taskLists";
import type { Widget } from "./types";

const rect = { x: 0, y: 0, w: 9, h: 8 };
const note: Widget = { id: "n", kind: "note", rect, text: "- [ ] a" };
const all: Widget = { id: "all", kind: "tasklist", rect, title: "" };
const atlas: Widget = { id: "atlas", kind: "tasklist", rect, title: "", project: "Atlas" };
const gone: Widget = { id: "gone", kind: "tasklist", rect, title: "", project: null };

describe("placeConvertedTaskList", () => {
  it("reuses a card already showing the conversion's project, whatever was created", () => {
    expect(placeConvertedTaskList([note, all, atlas], "Atlas", 0)).toEqual({ reuse: "atlas" });
    expect(placeConvertedTaskList([note, all, atlas], "Atlas", 3)).toEqual({ reuse: "atlas" });
    // No project lands on the All tasks list (no filter).
    expect(placeConvertedTaskList([note, atlas, all], null, 0)).toEqual({ reuse: "all" });
  });

  it("never reuses a card whose project was deleted, nor another project's card", () => {
    // A deleted project's card shows nothing — it is not the All tasks list.
    expect(placeConvertedTaskList([note, gone], null, 0)).toEqual({ add: "push" });
    expect(placeConvertedTaskList([note, all], "Atlas", 2)).toEqual({ add: "barrier" });
    // A note is never mistaken for a card.
    expect(placeConvertedTaskList([note], null, 1)).toEqual({ add: "barrier" });
  });

  it("adds a new card as a barrier only when tasks were written", () => {
    // Nothing new: nothing was written that undo can't take back, so the board's history is kept.
    expect(placeConvertedTaskList([note], "Atlas", 0)).toEqual({ add: "push" });
    expect(placeConvertedTaskList([note], "Atlas", 1)).toEqual({ add: "barrier" });
  });
});

describe("taskListDeleteCost", () => {
  it("names the project's page only when the list has a project", () => {
    expect(taskListDeleteCost("Atlas")).toBe(
      "Only the card goes — your tasks stay, on Atlas's project page and the calendar's Runway.",
    );
    const allTasks = taskListDeleteCost(undefined);
    expect(allTasks).toBe("Only the card goes — your tasks stay, on the calendar's Runway.");
    expect(allTasks).not.toMatch(/project page/);
  });

  it("says a deleted project's tasks have no project, with no page to point at", () => {
    const text = taskListDeleteCost(null);
    expect(text).not.toMatch(/project page/);
    expect(text).toMatch(/no project now/);
    expect(text).toMatch(/Runway/);
  });
});

describe("keepsNativeUndo", () => {
  /** A task list body: a `[data-native-undo]` region holding the add row's field, a tick, a button,
   *  a textarea and an inline contenteditable. Returned with a plain field outside the region. */
  function fixture() {
    document.body.innerHTML = `
      <div data-native-undo="">
        <input id="add" type="text" />
        <input id="search" type="search" />
        <input id="tick" type="checkbox" />
        <button id="row">Edit</button>
        <textarea id="notes"></textarea>
        <div id="rich" contenteditable="true"><span id="inner">x</span></div>
        <div contenteditable="true"><span id="frozen" contenteditable="false">y</span></div>
        <select id="pick"><option>a</option></select>
      </div>
      <input id="outside" type="text" />`;
    return (id: string) => document.getElementById(id);
  }

  it("keeps native undo for text fields inside the region", () => {
    const $ = fixture();
    expect(keepsNativeUndo($("add"))).toBe(true);
    expect(keepsNativeUndo($("search"))).toBe(true);
    expect(keepsNativeUndo($("notes"))).toBe(true);
    expect(keepsNativeUndo($("rich"))).toBe(true);
    expect(keepsNativeUndo($("inner"))).toBe(true);
  });

  it("leaves the board's undo on ticks, buttons and other non-text controls in the region", () => {
    const $ = fixture();
    expect(keepsNativeUndo($("tick"))).toBe(false);
    expect(keepsNativeUndo($("row"))).toBe(false);
    expect(keepsNativeUndo($("pick"))).toBe(false);
    expect(keepsNativeUndo($("frozen"))).toBe(false);
  });

  it("never opts out outside the region, or for a non-element target", () => {
    const $ = fixture();
    expect(keepsNativeUndo($("outside"))).toBe(false);
    expect(keepsNativeUndo(null)).toBe(false);
    expect(keepsNativeUndo(window)).toBe(false);
  });
});
