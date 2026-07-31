// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { atScrollEdge } from "./scrollAxis";

// The normaliser drives scrollers itself, so an unrecognised end edge is not a cosmetic miss: it
// applies a clamped, no-op delta and cancels the event, and the page under an exhausted list stops
// dead. Whether the edge is recognised comes down entirely to this predicate's epsilon, which is why
// it is the seam — the module's own behaviour needs real layout and cannot be unit-tested at all.
describe("atScrollEdge", () => {
  // A typical inner list: 500px tall, 1735px of content, so 1235px of travel.
  const CLIENT = 500;
  const SCROLL = 1735;

  it("is not an edge mid-scroll, in either direction", () => {
    expect(atScrollEdge(-100, 600, CLIENT, SCROLL)).toBe(false);
    expect(atScrollEdge(+100, 600, CLIENT, SCROLL)).toBe(false);
  });

  it("is the start edge at the top only for a wheel going up", () => {
    expect(atScrollEdge(-100, 0, CLIENT, SCROLL)).toBe(true);
    expect(atScrollEdge(+100, 0, CLIENT, SCROLL)).toBe(false);
  });

  it("is the end edge at an exact integer bottom", () => {
    expect(atScrollEdge(+100, SCROLL - CLIENT, CLIENT, SCROLL)).toBe(true);
  });

  it("is the end edge at a fractional-DPR bottom the scroller can never quite reach", () => {
    // THE regression. At 125%/150% zoom the extents are integers but the offset snaps to the device
    // pixel grid, so the largest reachable scrollTop sits ~1.6px short of the computed 1235. A 1px
    // epsilon calls that mid-scroll for ever: the wheel is swallowed and the page never chains.
    expect(atScrollEdge(+100, 1233.4, CLIENT, SCROLL)).toBe(true);
  });

  it("is never an edge for a zero delta", () => {
    expect(atScrollEdge(0, 0, CLIENT, SCROLL)).toBe(false);
    expect(atScrollEdge(0, SCROLL - CLIENT, CLIENT, SCROLL)).toBe(false);
  });

  it("is both edges on an element that cannot scroll at all", () => {
    // The wheel must always pass straight through such an element rather than being consumed by it.
    expect(atScrollEdge(-100, 0, CLIENT, CLIENT)).toBe(true);
    expect(atScrollEdge(+100, 0, CLIENT, CLIENT)).toBe(true);
  });

  it("gives the start edge no slack, so the last pixels stay reachable", () => {
    expect(atScrollEdge(-100, 1.5, CLIENT, SCROLL)).toBe(false);
  });
});
