// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The header's one control that changes a calendar (#884, C10): "+ New event" shows once a Google
// account is connected, and when no calendar can take an event it stays focusable and says why
// rather than going grey and silent.

import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", depth: "standard" }),
}));

import { CalendarHeader } from "./CalendarHeader";

afterEach(cleanup);

const REASON = "None of your calendars can take new events.";

function header(over: { onCreate?: () => void; createReason?: string | null } = {}) {
  render(
    <CalendarHeader
      view="week"
      availableViews={["month", "week", "day", "agenda"]}
      onViewChange={vi.fn()}
      range="day"
      onRangeChange={vi.fn()}
      dayCount={3}
      onDayCountChange={vi.fn()}
      customBounds={{}}
      onBoundsChange={vi.fn()}
      coords={null}
      label="October 2026"
      cursor={new Date(2026, 9, 12)}
      onPickDate={vi.fn()}
      onPrev={vi.fn()}
      onNext={vi.fn()}
      onToday={vi.fn()}
      onRefresh={vi.fn()}
      syncing={false}
      lastSync={null}
      {...over}
    />,
  );
}

describe("CalendarHeader", () => {
  it("New event is disabled with its reason when no calendar can take one", () => {
    const onCreate = vi.fn();
    header({ onCreate, createReason: REASON });
    const button = screen.getByRole<HTMLButtonElement>("button", { name: "+ New event" });
    // Still focusable, so a keyboard reaches the reason; marked, not disabled.
    expect(button.disabled).toBe(false);
    expect(button.getAttribute("aria-disabled")).toBe("true");
    const described = document.getElementById(button.getAttribute("aria-describedby") ?? "");
    expect(described?.textContent).toBe(REASON);
    expect(screen.getByRole("status").textContent).toBe("");
    // A click says why, and starts nothing.
    fireEvent.click(button);
    expect(onCreate).not.toHaveBeenCalled();
    expect(screen.getByRole("status").textContent).toBe(REASON);
  });

  it("New event starts one when a calendar can take it", () => {
    const onCreate = vi.fn();
    header({ onCreate, createReason: null });
    const button = screen.getByRole("button", { name: "+ New event" });
    expect(button.getAttribute("aria-disabled")).toBeNull();
    expect(button.getAttribute("aria-keyshortcuts")).toBe("c");
    fireEvent.click(button);
    expect(onCreate).toHaveBeenCalledTimes(1);
    expect(screen.queryByRole("status")).toBeNull();
  });

  it("has no New event without a Google account", () => {
    header();
    expect(screen.queryByRole("button", { name: "+ New event" })).toBeNull();
  });
});
