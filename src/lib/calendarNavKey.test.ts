// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import {
  calendarNavKey,
  calendarUndoKey,
  type NavKeyEvent,
  type UndoKeyEvent,
} from "./calendarNavKey";

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

  it("leaves Ctrl+Z to the Undo decision", () => {
    expect(calendarNavKey(key("z", { ctrlKey: true }), false)).toBeNull();
  });

  // Google's keys (#884, C10): c is the full editor, Shift+C the quick one.
  it("c creates, Shift+C quick-creates; not with Ctrl, in a field, or behind a dialog", () => {
    expect(calendarNavKey(key("c"), false)).toBe("create");
    expect(calendarNavKey(key("C", { shiftKey: true }), false)).toBe("quick-create");
    // Ctrl+C stays copy, and Cmd/Alt+C are the app's.
    expect(calendarNavKey(key("c", { ctrlKey: true }), false)).toBeNull();
    expect(calendarNavKey(key("c", { metaKey: true }), false)).toBeNull();
    expect(calendarNavKey(key("c", { altKey: true }), false)).toBeNull();
    for (const tag of ["INPUT", "TEXTAREA", "SELECT"]) {
      expect(calendarNavKey(key("c", { target: el(tag) }), false)).toBeNull();
    }
    expect(calendarNavKey(key("c", { target: el("DIV", true) }), false)).toBeNull();
    expect(calendarNavKey(key("c", { target: el("BUTTON") }), true)).toBeNull();
    expect(calendarNavKey(key("C", { shiftKey: true }), true)).toBeNull();
  });

  it("reads Shift, not the letter's case, so Caps Lock doesn't change the surface", () => {
    // Caps Lock on: the key reads "C" with no Shift held.
    expect(calendarNavKey(key("C"), false)).toBe("create");
    // Caps Lock on and Shift held: the key reads "c".
    expect(calendarNavKey(key("c", { shiftKey: true }), false)).toBe("quick-create");
  });
});

function undoKey(over: Partial<UndoKeyEvent> = {}): UndoKeyEvent {
  return {
    key: "z",
    code: "KeyZ",
    metaKey: false,
    ctrlKey: true,
    altKey: false,
    shiftKey: false,
    repeat: false,
    target: null,
    ...over,
  };
}

describe("calendarUndoKey", () => {
  it("is Ctrl+Z or Cmd+Z", () => {
    expect(calendarUndoKey(undoKey(), false)).toBe(true);
    expect(calendarUndoKey(undoKey({ ctrlKey: false, metaKey: true }), false)).toBe(true);
    // The key labelled Z, wherever the layout puts it (German, French) …
    expect(calendarUndoKey(undoKey({ code: "KeyY" }), false)).toBe(true);
    expect(calendarUndoKey(undoKey({ code: "KeyW" }), false)).toBe(true);
    // … and the physical Z key on a layout with no Latin Z.
    expect(calendarUndoKey(undoKey({ key: "я" }), false)).toBe(true);
  });

  it("is not a bare z, redo, Alt, a held key, or another key", () => {
    expect(calendarUndoKey(undoKey({ ctrlKey: false }), false)).toBe(false);
    expect(calendarUndoKey(undoKey({ shiftKey: true }), false)).toBe(false);
    expect(calendarUndoKey(undoKey({ altKey: true }), false)).toBe(false);
    expect(calendarUndoKey(undoKey({ repeat: true }), false)).toBe(false);
    expect(calendarUndoKey(undoKey({ code: "KeyY", key: "y" }), false)).toBe(false);
    expect(calendarUndoKey(undoKey({ code: "KeyZ", key: "y" }), false)).toBe(false); // German Y
  });

  it("leaves a field its own Undo, and stands down behind a dialog", () => {
    for (const tag of ["INPUT", "TEXTAREA", "SELECT"]) {
      expect(calendarUndoKey(undoKey({ target: el(tag) }), false)).toBe(false);
    }
    expect(calendarUndoKey(undoKey({ target: el("DIV", true) }), false)).toBe(false);
    expect(calendarUndoKey(undoKey({ target: el("BUTTON") }), true)).toBe(false);
    expect(calendarUndoKey(undoKey({ target: el("BUTTON") }), false)).toBe(true);
  });
});
