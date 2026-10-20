// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The date field's earliest allowed date (`min`): a date before it, typed or picked, becomes it, and
// the picker greys the days before it. Without `min` nothing changes. And `onAccept`: only a date
// typed or picked is given, never text the field showed on its own.

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

// A form holding a refusal (`onReject`) lets it go on `onAccept`. Its save blurs the focused field
// first, so a blur on the date the field put back must not pass for one given.
describe("DateField's onAccept", () => {
  it("fires for a date typed, even the one it holds, never for text nobody typed", () => {
    const onAccept = vi.fn();
    const onReject = vi.fn();
    render(
      <DateField
        value="2026-10-14"
        onCommit={() => {}}
        onReject={onReject}
        onAccept={onAccept}
        ariaLabel="Starts"
      />,
    );
    const input = screen.getByLabelText<HTMLInputElement>("Starts");
    // In and out on its own date.
    input.focus();
    fireEvent.blur(input);
    expect(onAccept).not.toHaveBeenCalled();
    // Refused, then in and out on the date it put back, by blur and by Enter.
    typeInto(input, "31-11-2026");
    expect(onReject).toHaveBeenCalledWith("31-11-2026");
    expect(input.value).toBe("14-10-2026");
    input.focus();
    fireEvent.blur(input);
    input.focus();
    fireEvent.keyDown(input, { key: "Enter" });
    expect(onAccept).not.toHaveBeenCalled();
    // The date it shows, typed over (a change to the same text is no change at all).
    input.focus();
    fireEvent.change(input, { target: { value: "" } });
    typeInto(input, "14-10-2026");
    expect(onAccept).toHaveBeenCalledTimes(1);
  });

  it("fires for a date picked", () => {
    const onAccept = vi.fn();
    render(
      <DateField value="2026-10-14" onCommit={() => {}} onAccept={onAccept} ariaLabel="Starts" />,
    );
    fireEvent.click(screen.getByLabelText("Starts"));
    fireEvent.click(screen.getByRole("button", { name: "Today" }));
    expect(onAccept).toHaveBeenCalledTimes(1);
  });

  it("stays quiet when the date changed underneath what was typed", () => {
    const onAccept = vi.fn();
    const field = (value: string) => (
      <DateField value={value} onCommit={() => {}} onAccept={onAccept} ariaLabel="Starts" />
    );
    const { rerender } = render(field("2026-10-14"));
    const input = screen.getByLabelText<HTMLInputElement>("Starts");
    input.focus();
    fireEvent.change(input, { target: { value: "15-10-2026" } });
    // The form moved it meanwhile: the typing is gone, and what shows is the form's own.
    rerender(field("2026-10-20"));
    expect(input.value).toBe("20-10-2026");
    fireEvent.blur(input);
    expect(onAccept).not.toHaveBeenCalled();
  });
});
