// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Popover's stacking contract. An `escapeClipping` panel is portalled to `document.body` and
// positioned `fixed`, which takes it out of its parent's stacking context — so its z-index is no
// longer compared against its siblings but against everything in the page, `Modal`'s `z-50`
// included. At the shared `z-30` it painted BEHIND the dialog surface: the date picker inside a
// pinboard folder set to "Overlay" opened completely invisibly, and because the panel was mounted
// and merely covered, the next click landed on the scrim and read as an outside-dismissal.
//
// jsdom cannot see paint order, so these assert the classes rather than the pixels — which is also
// the sharper test for the second half of the bug: `cn` is a plain joiner, NOT tailwind-merge, so a
// fix that appended `z-[60]` beside a constant `z-30` would leave BOTH on the element and let the
// generated stylesheet's ordering decide the winner. "Exactly one z-utility" is the property that
// has to hold, and only a class-level assertion can state it.

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it } from "vitest";

import { Popover } from "./Popover";

// jsdom ships no ResizeObserver, and the escape-clipping branch constructs one to re-place the panel
// when it resizes. Placement is not what these assert — the class list is — so a no-op is enough.
beforeAll(() => {
  if (!("ResizeObserver" in globalThis)) {
    (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
});

afterEach(cleanup);

/** Every z-index utility on the panel, in the order they appear in `className`. */
function zClasses(el: HTMLElement): string[] {
  return Array.from(el.classList).filter((c) => /^z-(\[|\d)/.test(c));
}

function open(escapeClipping: boolean) {
  render(
    <Popover
      escapeClipping={escapeClipping}
      ariaLabel="Pick a date"
      trigger={({ toggle }) => (
        <button type="button" onClick={toggle}>
          Open
        </button>
      )}
    >
      <span>panel body</span>
    </Popover>,
  );
  fireEvent.click(screen.getByText("Open"));
  return screen.getByRole("group", { name: "Pick a date" });
}

describe("Popover stacking", () => {
  it("lifts a clipping-escaped panel above Modal's scrim", () => {
    // The regression. `z-[60]` is the tier HelpOverlay already uses for exactly this reason
    // ("above Modal's z-50"), so this is the established rung rather than a new one.
    const panel = open(true);
    expect(panel.className).toContain("fixed");
    expect(zClasses(panel)).toEqual(["z-[60]"]);
  });

  it("leaves an anchored panel at z-30", () => {
    // The other half, and the reason the lift is conditional rather than global: an anchored panel
    // lives inside its parent's stacking context. Raising it too would let a popover in the page
    // punch through overlays that are meant to cover it.
    const panel = open(false);
    expect(panel.className).toContain("absolute");
    expect(zClasses(panel)).toEqual(["z-30"]);
  });

  it("never emits two z-index utilities", () => {
    // `cn` does not de-conflict. Two z-classes on one element is a silent, stylesheet-ordering-
    // dependent bug, so pin the count directly for both modes.
    for (const escapeClipping of [true, false]) {
      const panel = open(escapeClipping);
      expect(zClasses(panel)).toHaveLength(1);
      cleanup();
    }
  });
});
