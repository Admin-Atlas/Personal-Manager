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
