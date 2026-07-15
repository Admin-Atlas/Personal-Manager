// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import {
  BUCKET_MS,
  canRedo,
  canUndo,
  commit,
  commitBarrier,
  commitSilent,
  initHistory,
  redo,
  resetHistory,
  undo,
} from "./history";

/** A stand-in board: `t` is the "text" the byte budget weighs. */
interface Doc {
  t: string;
}
const doc = (t: string): Doc => ({ t });
const weigh = (d: Doc) => d.t.length;

describe("commit — the basics", () => {
  it("records the previous value and clears redo", () => {
    let h = initHistory(doc("a"));
    h = commit(h, doc("b"), { now: 0 });
    expect(h.present).toEqual(doc("b"));
    expect(canUndo(h)).toBe(true);
    h = undo(h);
    expect(h.present).toEqual(doc("a"));
    expect(canRedo(h)).toBe(true);
    // A fresh change after an undo abandons the redo branch.
    h = commit(h, doc("c"), { now: 0 });
    expect(canRedo(h)).toBe(false);
  });

  it("is a no-op when the value is unchanged by identity", () => {
    // grid's unchanged-landing and raiseWidget's already-on-top both hand back the same object; a
    // click that moved nothing must not cost an undo step.
    const same = doc("a");
    const h = initHistory(same);
    expect(commit(h, same, { now: 0 })).toBe(h);
  });

  it("round-trips undo → redo", () => {
    let h = initHistory(doc("a"));
    h = commit(h, doc("b"), { now: 0 });
    h = commit(h, doc("c"), { now: 9999 });
    h = undo(undo(h));
    expect(h.present).toEqual(doc("a"));
    h = redo(redo(h));
    expect(h.present).toEqual(doc("c"));
    expect(canRedo(h)).toBe(false);
  });

  it("undo/redo at the ends are no-ops", () => {
    const h = initHistory(doc("a"));
    expect(undo(h)).toBe(h);
    expect(redo(h)).toBe(h);
  });
});

describe("commit — bucket coalescing", () => {
  it("merges same-key changes inside the bucket into ONE step", () => {
    let h = initHistory(doc(""));
    h = commit(h, doc("h"), { key: "text:1", now: 0 });
    h = commit(h, doc("he"), { key: "text:1", now: 500 });
    h = commit(h, doc("hel"), { key: "text:1", now: 900 });
    expect(h.past).toHaveLength(1);
    expect(undo(h).present).toEqual(doc("")); // all the way back to before the run
  });

  it("closes the bucket on AGE, not on the gap since the last change", () => {
    // The distinction that matters: typing steadily with no pause must still break into steps, or
    // one Ctrl+Z would wipe a whole paragraph. Every keystroke here is 500ms after the last — never
    // idle — yet the 3s-old bucket still closes.
    let h = initHistory(doc(""));
    for (let i = 1; i <= 10; i++)
      h = commit(h, doc("x".repeat(i)), { key: "text:1", now: i * 500 });
    expect(h.past.length).toBeGreaterThan(1);
  });

  it("merges at the bucket edge and splits just past it", () => {
    let a = initHistory(doc(""));
    a = commit(a, doc("x"), { key: "text:1", now: 0 });
    a = commit(a, doc("xy"), { key: "text:1", now: BUCKET_MS - 1 });
    expect(a.past).toHaveLength(1);

    let b = initHistory(doc(""));
    b = commit(b, doc("x"), { key: "text:1", now: 0 });
    b = commit(b, doc("xy"), { key: "text:1", now: BUCKET_MS });
    expect(b.past).toHaveLength(2);
  });

  it("never merges across different keys", () => {
    // A colour click happens mid-edit (the swatches only show while editing), so it must not be
    // swallowed by the text bucket around it.
    let h = initHistory(doc("a"));
    h = commit(h, doc("ab"), { key: "text:1", now: 0 });
    h = commit(h, doc("ab!"), { key: "color:1", now: 100 });
    h = commit(h, doc("abc!"), { key: "text:1", now: 200 });
    expect(h.past).toHaveLength(3);
  });

  it("never merges a null key, in either direction", () => {
    let h = initHistory(doc("a"));
    h = commit(h, doc("b"), { key: null, now: 0 });
    h = commit(h, doc("c"), { key: null, now: 1 });
    expect(h.past).toHaveLength(2);
  });

  it("does not merge two different widgets' text", () => {
    let h = initHistory(doc("a"));
    h = commit(h, doc("b"), { key: "text:1", now: 0 });
    h = commit(h, doc("c"), { key: "text:2", now: 10 });
    expect(h.past).toHaveLength(2);
  });

  it("a redone step is never merged into", () => {
    let h = initHistory(doc(""));
    h = commit(h, doc("x"), { key: "text:1", now: 0 });
    h = undo(h);
    h = redo(h);
    h = commit(h, doc("xy"), { key: "text:1", now: 1 });
    expect(h.past).toHaveLength(2); // the replayed step stands on its own
    expect(undo(h).present).toEqual(doc("x"));
  });
});

describe("commitSilent / commitBarrier", () => {
  it("silent moves the present without adding a step", () => {
    let h = initHistory(doc("a"));
    h = commit(h, doc("b"), { now: 0 });
    h = commitSilent(h, doc("b-raised"));
    expect(h.present).toEqual(doc("b-raised"));
    expect(h.past).toHaveLength(1);
    // The silent change is carried INTO the next entry, so undo doesn't resurrect the pre-raise board.
    h = commit(h, doc("c"), { now: 0 });
    expect(undo(h).present).toEqual(doc("b-raised"));
  });

  it("silent is a no-op on an unchanged value", () => {
    const same = doc("a");
    const h = initHistory(same);
    expect(commitSilent(h, same)).toBe(h);
  });

  it("barrier drops past AND future", () => {
    let h = initHistory(doc("a"));
    h = commit(h, doc("b"), { now: 0 });
    h = undo(h);
    expect(canRedo(h)).toBe(true);
    h = commitBarrier(h, doc("linked"));
    expect(h.present).toEqual(doc("linked"));
    expect(canUndo(h)).toBe(false);
    expect(canRedo(h)).toBe(false);
  });
});

describe("resetHistory", () => {
  it("drops everything — a load has nothing behind it", () => {
    let h = initHistory(doc("a"));
    h = commit(h, doc("b"), { now: 0 });
    expect(canUndo(h)).toBe(true); // there IS something to drop
    h = resetHistory(doc("loaded"));
    expect(h.present).toEqual(doc("loaded"));
    // Crucially: the first Ctrl+Z after a load can't restore the empty default the board started as.
    expect(canUndo(h)).toBe(false);
    expect(canRedo(h)).toBe(false);
  });
});

describe("trim — the stack is budgeted in bytes, not entries", () => {
  it("drops the oldest entries once retained text exceeds the budget", () => {
    // 20 steps over a steady 100-char note: without a byte cap that's 20 entries retained.
    let h = initHistory(doc(""));
    for (let i = 1; i <= 20; i++) {
      h = commit(h, doc("x".repeat(100) + i), { key: null, now: i, weigh, budget: 500 });
    }
    const retained = h.past.reduce((n, e) => n + e.value.t.length, 0);
    expect(retained).toBeLessThanOrEqual(500);
    expect(h.past.length).toBeGreaterThan(0);
    // The entries kept are the RECENT ones — the far end of the stack is what you're least likely to
    // reach for, so that's what goes.
    expect(h.past[h.past.length - 1].value.t).toContain("19");
  });

  it("keeps one entry even when a single value is over budget", () => {
    // A pasted novel is still worth one undo.
    let h = initHistory(doc(""));
    h = commit(h, doc("x".repeat(5000)), { key: null, now: 0, weigh, budget: 10 });
    h = commit(h, doc("y".repeat(5000)), { key: null, now: 1, weigh, budget: 10 });
    expect(h.past).toHaveLength(1);
    expect(canUndo(h)).toBe(true);
  });

  it("enforces a hard entry ceiling", () => {
    let h = initHistory(doc(""));
    for (let i = 0; i < 20; i++) h = commit(h, doc(`v${i}`), { key: null, now: i, maxEntries: 5 });
    expect(h.past).toHaveLength(5);
  });

  it("without a weigh function the byte budget never bites", () => {
    let h = initHistory(doc(""));
    for (let i = 1; i <= 5; i++) h = commit(h, doc("x".repeat(i * 1000)), { key: null, now: i });
    expect(h.past).toHaveLength(5);
  });
});
