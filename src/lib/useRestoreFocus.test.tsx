// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The non-modal half of the focus contract: remember the opener, hand focus back only when asked.
// The third case is the latent bug this extraction fixes — the version in Popover re-captured
// inside an effect keyed on a callback identity, so a parent re-render while the panel was open
// could re-point "the opener" at something inside the panel that was about to unmount.

import { cleanup, fireEvent, render } from "@testing-library/react";
import { useState } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { useRestoreFocus } from "./useRestoreFocus";

afterEach(cleanup);

function Harness({ tick = 0 }: { tick?: number }) {
  const [open, setOpen] = useState(false);
  const restore = useRestoreFocus(open);
  return (
    <>
      <button data-testid="opener" onClick={() => setOpen(true)}>
        open {tick}
      </button>
      {open && (
        <div data-testid="panel">
          <button data-testid="inside">inside</button>
          <button
            data-testid="close"
            onClick={() => {
              setOpen(false);
              restore();
            }}
          >
            close
          </button>
        </div>
      )}
    </>
  );
}

describe("useRestoreFocus", () => {
  it("hands focus back to the element that was focused when the panel opened", () => {
    const { getByTestId } = render(<Harness />);
    const opener = getByTestId("opener");
    opener.focus();
    fireEvent.click(opener);
    getByTestId("inside").focus();
    fireEvent.click(getByTestId("close"));
    expect(document.activeElement).toBe(opener);
  });

  it("does nothing when nothing was captured", () => {
    const { getByTestId } = render(<Harness />);
    // Never focused the opener, so `document.activeElement` was <body> — restoring must not throw
    // and must not steal focus from wherever it ended up.
    fireEvent.click(getByTestId("opener"));
    expect(() => fireEvent.click(getByTestId("close"))).not.toThrow();
  });

  it("does not re-capture while the panel stays open", () => {
    const { getByTestId, rerender } = render(<Harness tick={0} />);
    const opener = getByTestId("opener");
    opener.focus();
    fireEvent.click(opener);
    // Focus moves inside the panel, then the parent re-renders — the opener must not follow it.
    getByTestId("inside").focus();
    rerender(<Harness tick={1} />);
    expect(document.activeElement).toBe(getByTestId("inside"));
    fireEvent.click(getByTestId("close"));
    expect(document.activeElement).toBe(getByTestId("opener"));
  });
});

// A singleton panel that is re-POINTED at a new opener without closing in between — the calendar's
// event popover, driven from selection state. Keyed re-capture is opt-in precisely because it is
// wrong for Popover, where a re-render must never move "the opener" (the case above).
function KeyedHarness({ openerKey }: { openerKey: string }) {
  const restore = useRestoreFocus(true, openerKey);
  return (
    <>
      <button data-testid="chip-a">A</button>
      <button data-testid="chip-b">B</button>
      <div data-testid="panel">
        <button data-testid="close" onClick={restore}>
          close
        </button>
      </div>
    </>
  );
}

// The harness renders the chips itself, so the very first mount can only ever capture <body> —
// nothing is focused yet. Every case below therefore establishes its opener the way the real panel
// does: focus a chip, then re-key.
describe("useRestoreFocus — keyed to the opener", () => {
  it("re-captures when the key changes, so the panel points at the CURRENT opener", () => {
    const { getByTestId, rerender } = render(<KeyedHarness openerKey="0" />);
    getByTestId("chip-a").focus();
    rerender(<KeyedHarness openerKey="a" />);
    // Opened from A, then re-pointed at B without ever closing.
    getByTestId("chip-b").focus();
    rerender(<KeyedHarness openerKey="b" />);

    getByTestId("close").focus();
    fireEvent.click(getByTestId("close"));
    expect(document.activeElement).toBe(getByTestId("chip-b"));
  });

  it("does not re-capture on a re-render that keeps the same key", () => {
    const { getByTestId, rerender } = render(<KeyedHarness openerKey="0" />);
    getByTestId("chip-a").focus();
    rerender(<KeyedHarness openerKey="a" />);
    // A re-render while focus sits inside the panel must not re-point the opener at the panel's own
    // button — the same latent bug the unkeyed case guards against.
    getByTestId("close").focus();
    rerender(<KeyedHarness openerKey="a" />);

    fireEvent.click(getByTestId("close"));
    expect(document.activeElement).toBe(getByTestId("chip-a"));
  });
});
