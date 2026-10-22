// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// While a choice is on its way (`busy`, first used by the reply to an invitation, #884 C14b) every
// option says it's unavailable and a press does nothing, but none is `disabled`: that would drop
// the focus of the one just pressed, outside a non-modal popover, to the page.

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SegmentedControl } from "./SegmentedControl";

afterEach(cleanup);

const OPTIONS = [
  { value: "a", label: "A" },
  { value: "b", label: "B" },
] as const;

describe("SegmentedControl", () => {
  it("while busy, ignores a press and keeps focus on the option pressed", () => {
    const onChange = vi.fn();
    const { rerender } = render(
      <SegmentedControl ariaLabel="Pick" options={OPTIONS} value="a" onChange={onChange} busy />,
    );
    const b = screen.getByRole<HTMLButtonElement>("button", { name: "B" });
    b.focus();
    fireEvent.click(b);
    expect(onChange).not.toHaveBeenCalled();
    expect(b.getAttribute("aria-disabled")).toBe("true");
    expect(b.disabled).toBe(false);
    expect(document.activeElement).toBe(b);
    rerender(<SegmentedControl ariaLabel="Pick" options={OPTIONS} value="a" onChange={onChange} />);
    expect(b.getAttribute("aria-disabled")).toBeNull();
    fireEvent.click(b);
    expect(onChange).toHaveBeenCalledWith("b");
  });
});
