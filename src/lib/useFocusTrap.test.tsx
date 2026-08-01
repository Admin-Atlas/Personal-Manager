// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The keyboard-focus contract for dialogs (A11y foundation PR1): focus enters the container on open,
// Tab/Shift+Tab wrap at the edges, and focus returns to the opener on close. jsdom never moves focus
// on a real Tab press, so the trap is driven the same way it works in production — it listens for
// keydown and calls `.focus()` itself — which is exactly what these assertions exercise.

import { cleanup, fireEvent, render } from "@testing-library/react";
import { useRef, useState } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { useFocusTrap } from "./useFocusTrap";

// This repo registers no global testing-library cleanup (vitest.config has no setup file), so
// unmount between tests here — otherwise repeated renders pile identical test-ids into document.body.
afterEach(cleanup);

function Harness() {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useFocusTrap(open, ref);
  return (
    <>
      <button data-testid="opener" onClick={() => setOpen((o) => !o)}>
        toggle
      </button>
      {open && (
        <div ref={ref} tabIndex={-1} data-testid="dialog">
          <button data-testid="first">first</button>
          <button data-testid="mid">mid</button>
          <button data-testid="last">last</button>
        </div>
      )}
    </>
  );
}

/** Focus the opener (so it's the element to restore to) and open the dialog. */
function openDialog(getByTestId: (id: string) => HTMLElement): HTMLElement {
  const opener = getByTestId("opener");
  opener.focus();
  fireEvent.click(opener);
  return opener;
}

describe("useFocusTrap", () => {
  it("moves focus to the first focusable element on open", () => {
    const { getByTestId } = render(<Harness />);
    openDialog(getByTestId);
    expect(document.activeElement).toBe(getByTestId("first"));
  });

  it("wraps Tab from the last element back to the first", () => {
    const { getByTestId } = render(<Harness />);
    openDialog(getByTestId);
    const last = getByTestId("last");
    last.focus();
    fireEvent.keyDown(last, { key: "Tab" });
    expect(document.activeElement).toBe(getByTestId("first"));
  });

  it("wraps Shift+Tab from the first element back to the last", () => {
    const { getByTestId } = render(<Harness />);
    openDialog(getByTestId);
    const first = getByTestId("first");
    first.focus();
    fireEvent.keyDown(first, { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(getByTestId("last"));
  });

  it("does not intercept a non-Tab key", () => {
    const { getByTestId } = render(<Harness />);
    openDialog(getByTestId);
    const mid = getByTestId("mid");
    mid.focus();
    fireEvent.keyDown(mid, { key: "ArrowDown" });
    expect(document.activeElement).toBe(mid);
  });

  it("restores focus to the opener when the container closes", () => {
    const { getByTestId, queryByTestId } = render(<Harness />);
    const opener = openDialog(getByTestId);
    expect(document.activeElement).toBe(getByTestId("first"));
    fireEvent.click(opener); // toggle closed → dialog unmounts → focus restored
    expect(queryByTestId("dialog")).toBeNull();
    expect(document.activeElement).toBe(opener);
  });
});

// Nested dialogs. Modal does not portal, so a dialog opened from inside another one is a DOM
// DESCENDANT of it and the same bubbling keydown reaches both traps. This is what makes
// Settings-as-a-Modal safe: Settings contains its own unsaved-changes guard, the per-tab reset
// confirmations, the re-index progress and the three remove-my-data steps.

function NestedHarness({ innerButtons = 2 }: { innerButtons?: number }) {
  const outer = useRef<HTMLDivElement>(null);
  const inner = useRef<HTMLDivElement>(null);
  useFocusTrap(true, outer);
  useFocusTrap(true, inner);
  return (
    <div ref={outer} role="dialog" aria-modal="true" tabIndex={-1} data-testid="outer">
      <button data-testid="outer-first">outer first</button>
      <button data-testid="outer-last">outer last</button>
      <div ref={inner} role="dialog" aria-modal="true" tabIndex={-1} data-testid="inner">
        <button data-testid="inner-first">inner first</button>
        {innerButtons > 1 && <button data-testid="inner-last">inner last</button>}
      </div>
    </div>
  );
}

describe("useFocusTrap with a nested dialog", () => {
  it("keeps Tab inside a one-button nested dialog", () => {
    // The case where the outer trap does real damage. A confirmation whose only focusable is its
    // Confirm button, rendered last inside the dialog behind it: the inner trap refocuses its one
    // button, and the outer trap — whose own focusable list ENDS with that same button — then reads
    // it as "the last element" and wraps focus to the outer dialog's first control. One Tab and the
    // user is behind the confirmation they were answering.
    const { getByTestId } = render(<NestedHarness innerButtons={1} />);
    const only = getByTestId("inner-first");
    only.focus();
    fireEvent.keyDown(only, { key: "Tab" });
    expect(document.activeElement).toBe(only);
    expect(document.activeElement).not.toBe(getByTestId("outer-first"));
  });

  it("wraps Tab against the inner dialog's own focusables", () => {
    const { getByTestId } = render(<NestedHarness />);
    const innerLast = getByTestId("inner-last");
    innerLast.focus();
    fireEvent.keyDown(innerLast, { key: "Tab" });
    expect(document.activeElement).toBe(getByTestId("inner-first"));
  });

  it("wraps Shift+Tab against the inner dialog too", () => {
    const { getByTestId } = render(<NestedHarness />);
    const innerFirst = getByTestId("inner-first");
    innerFirst.focus();
    fireEvent.keyDown(innerFirst, { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(getByTestId("inner-last"));
  });

  it("leaves the outer trap in charge while focus is outside the nested dialog", () => {
    const { getByTestId } = render(<NestedHarness />);
    // Focus outside the inner dialog: the stand-down rule must not apply, so the outer trap still
    // wraps at its own edge (its last focusable happens to be the inner dialog's last button).
    const outerFirst = getByTestId("outer-first");
    outerFirst.focus();
    fireEvent.keyDown(outerFirst, { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(getByTestId("inner-last"));
  });
});

// A dialog whose interactive children are deliberately NOT tab stops. The command palette is the
// one in the tree: its result rows are `<button tabIndex={-1}>`, selected through
// `aria-activedescendant` from the input rather than by tabbing.

function ActivedescendantHarness() {
  const ref = useRef<HTMLDivElement>(null);
  useFocusTrap(true, ref);
  return (
    <div ref={ref} role="dialog" aria-modal="true" tabIndex={-1} data-testid="dialog">
      <input data-testid="query" />
      <button data-testid="row-1" tabIndex={-1}>
        row 1
      </button>
      <button data-testid="row-2" tabIndex={-1}>
        row 2
      </button>
    </div>
  );
}

describe("useFocusTrap with tabindex=-1 children", () => {
  it("counts only TAB-REACHABLE elements, so Tab cannot walk out of the dialog", () => {
    // With `tabindex="-1"` elements counted as stops, the input was neither first nor last, the
    // trap declined to act, and the browser's own Tab skipped every row and left the dialog — the
    // exact escape that made the palette's Escape handler unreachable before it wore Modal.
    const { getByTestId } = render(<ActivedescendantHarness />);
    const query = getByTestId("query");
    expect(document.activeElement).toBe(query);

    fireEvent.keyDown(query, { key: "Tab" });
    expect(document.activeElement).toBe(query);
    fireEvent.keyDown(query, { key: "Tab", shiftKey: true });
    expect(document.activeElement).toBe(query);
  });
});
