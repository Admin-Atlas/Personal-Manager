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
