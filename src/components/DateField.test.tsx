// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The date field's earliest allowed date (`min`): a date before it, typed or picked, becomes it, and
// the picker greys the days before it. Without `min` nothing changes.

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import { DateField } from "./DateField";

vi.mock("../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate" }),
}));

// jsdom ships no ResizeObserver, and the picker's popover (escapeClipping) builds one to re-place
// itself; placement isn't what these assert.
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

function typeInto(input: HTMLInputElement, text: string) {
  input.focus();
  fireEvent.change(input, { target: { value: text } });
  fireEvent.blur(input);
}

describe("DateField with an earliest date", () => {
  it("turns an earlier typed date into the earliest one", () => {
    const onCommit = vi.fn();
    render(<DateField value="2026-10-14" min="2026-10-12" onCommit={onCommit} ariaLabel="Ends" />);
    const input = screen.getByLabelText<HTMLInputElement>("Ends");
    typeInto(input, "05-10-2026");
    expect(onCommit).toHaveBeenCalledWith("2026-10-12");
    expect(input.value).toBe("12-10-2026");
  });

  it("commits a date on or after it as typed", () => {
    const onCommit = vi.fn();
    render(<DateField value="2026-10-14" min="2026-10-12" onCommit={onCommit} ariaLabel="Ends" />);
    typeInto(screen.getByLabelText<HTMLInputElement>("Ends"), "12-10-2026");
    expect(onCommit).toHaveBeenCalledWith("2026-10-12");
  });

  it("greys and disables the days before it in the picker", () => {
    render(<DateField value="2026-10-14" min="2026-10-12" onCommit={() => {}} ariaLabel="Ends" />);
    fireEvent.click(screen.getByLabelText("Ends"));
    // Day buttons are named "<weekday> dd-mm", with the year only outside the current one.
    const day = (n: string) =>
      screen
        .getAllByRole("button")
        .find((b) =>
          / \d\d-\d\d(-\d{4})?$/.test(b.getAttribute("aria-label") ?? "")
            ? (b.getAttribute("aria-label") ?? "").includes(` ${n}-10`)
            : false,
        ) as HTMLButtonElement;
    expect(day("11").disabled).toBe(true);
    expect(day("12").disabled).toBe(false);
  });

  it("leaves every date alone without one", () => {
    const onCommit = vi.fn();
    render(<DateField value="2026-10-14" onCommit={onCommit} ariaLabel="Ends" />);
    typeInto(screen.getByLabelText<HTMLInputElement>("Ends"), "05-10-2026");
    expect(onCommit).toHaveBeenCalledWith("2026-10-05");
  });
});
