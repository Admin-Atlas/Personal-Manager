// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The header's one control that changes a calendar (#884, C10): "+ New event" shows once a Google
// account is connected, and when no calendar can take an event it stays focusable and says why
// rather than going grey and silent.

import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";

vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", depth: "standard" }),
}));

import { CalendarHeader } from "./CalendarHeader";

afterEach(cleanup);

const REASON = "None of your calendars can take new events.";

function header(over: { onCreate?: () => void; createReason?: string | null } = {}) {
  return render(element(over));
}

function element(over: { onCreate?: () => void; createReason?: string | null } = {}) {
  return (
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
    />
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

  // One primary button on screen: the editor's Save (C1-L5).
  it("New event is a secondary button", () => {
    for (const createReason of [null, REASON]) {
      header({ onCreate: vi.fn(), createReason });
      const button = screen.getByRole("button", { name: "+ New event" });
      expect(button.getAttribute("data-variant")).toBe("secondary");
      cleanup();
    }
  });

  it("says why for six seconds; another click starts over and says it anew", () => {
    vi.useFakeTimers();
    try {
      header({ onCreate: vi.fn(), createReason: REASON });
      const button = screen.getByRole("button", { name: "+ New event" });
      const status = () => screen.getByRole("status");
      const pass = (ms: number) =>
        act(() => {
          vi.advanceTimersByTime(ms);
        });
      fireEvent.click(button);
      const first = status().firstElementChild;
      expect(status().textContent).toBe(REASON);
      pass(4000);
      fireEvent.click(button);
      // New text for a screen reader to say, not the same text left in place.
      expect(status().firstElementChild).not.toBeNull();
      expect(status().firstElementChild).not.toBe(first);
      // Six seconds from the second click, not the first.
      pass(4000);
      expect(status().textContent).toBe(REASON);
      pass(2000);
      expect(status().textContent).toBe("");
    } finally {
      vi.useRealTimers();
    }
  });

  it("forgets a reason said once a calendar can take an event, so the next waits for a click", () => {
    const onCreate = vi.fn();
    const view = header({ onCreate, createReason: REASON });
    fireEvent.click(screen.getByRole("button", { name: "+ New event" }));
    expect(screen.getByRole("status").textContent).toBe(REASON);
    view.rerender(element({ onCreate, createReason: null }));
    view.rerender(element({ onCreate, createReason: REASON }));
    expect(screen.getByRole("status").textContent).toBe("");
  });

  it("has no New event without a Google account", () => {
    header();
    expect(screen.queryByRole("button", { name: "+ New event" })).toBeNull();
  });
});
