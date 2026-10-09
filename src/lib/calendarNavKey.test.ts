// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import { calendarNavKey, type NavKeyEvent } from "./calendarNavKey";

function key(k: string, over: Partial<NavKeyEvent> = {}): NavKeyEvent {
  return { key: k, metaKey: false, ctrlKey: false, altKey: false, target: null, ...over };
}

function el(tagName: string, isContentEditable = false): EventTarget {
  return { tagName, isContentEditable } as unknown as EventTarget;
}

describe("calendarNavKey", () => {
  it("steps and jumps on the bare keys", () => {
    expect(calendarNavKey(key("ArrowLeft"), false)).toBe("prev");
    expect(calendarNavKey(key("ArrowRight"), false)).toBe("next");
    expect(calendarNavKey(key("t"), false)).toBe("today");
    expect(calendarNavKey(key("T"), false)).toBe("today");
    expect(calendarNavKey(key("x"), false)).toBeNull();
  });

  // The bug: the listener is on `window`, so focus on a dialog's button paged the grid behind it.
  it("does nothing while a dialog is open, even with focus on a button", () => {
    for (const k of ["ArrowLeft", "ArrowRight", "t"]) {
      expect(calendarNavKey(key(k, { target: el("BUTTON") }), true)).toBeNull();
    }
    expect(calendarNavKey(key("ArrowLeft", { target: el("BUTTON") }), false)).toBe("prev");
  });

  it("leaves app shortcuts and typing alone", () => {
    expect(calendarNavKey(key("ArrowLeft", { metaKey: true }), false)).toBeNull();
    expect(calendarNavKey(key("ArrowRight", { ctrlKey: true }), false)).toBeNull();
    expect(calendarNavKey(key("t", { altKey: true }), false)).toBeNull();
    for (const tag of ["INPUT", "TEXTAREA", "SELECT"]) {
      expect(calendarNavKey(key("ArrowLeft", { target: el(tag) }), false)).toBeNull();
    }
    expect(calendarNavKey(key("t", { target: el("DIV", true) }), false)).toBeNull();
  });
});
