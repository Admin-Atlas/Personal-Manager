// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// `isDialogOpenOutside`: a view's own dialogs (the Pinboard's folder overlay) don't count against its
// shortcuts; a dialog opened over the whole view (Settings, the command palette) does.

import { cleanup, render } from "@testing-library/react";
import { useRef } from "react";
import { afterEach, describe, expect, it } from "vitest";
import { isAnyDialogOpen, isDialogOpenOutside, useDialogLayer } from "./useDialogLayer";

afterEach(cleanup);

function Dialog({ id }: { id: string }) {
  const ref = useRef<HTMLDivElement>(null);
  useDialogLayer(true, ref);
  return <div ref={ref} data-testid={id} />;
}

describe("isDialogOpenOutside", () => {
  it("is false with no dialog open", () => {
    render(<div data-testid="view" />);
    expect(isAnyDialogOpen()).toBe(false);
    expect(isDialogOpenOutside(document.querySelector("[data-testid=view]"))).toBe(false);
  });

  it("doesn't count a view's own dialog, and counts one opened over it", () => {
    const { getByTestId, rerender } = render(
      <>
        <div data-testid="view">
          <Dialog id="folder" />
        </div>
      </>,
    );
    expect(isDialogOpenOutside(getByTestId("view"))).toBe(false);
    rerender(
      <>
        <div data-testid="view">
          <Dialog id="folder" />
        </div>
        <Dialog id="settings" />
      </>,
    );
    expect(isDialogOpenOutside(getByTestId("view"))).toBe(true);
    // No view to judge by: any open dialog counts.
    expect(isDialogOpenOutside(null)).toBe(true);
  });
});
