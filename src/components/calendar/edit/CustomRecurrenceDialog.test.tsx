// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The Custom recurrence dialog (#884, C11 §7): what it can't make is said in the backend's words and
// Done waits (more than 730 times, an end date before the start or one it couldn't read, no weekday);
// Done hands the rule back, Cancel nothing; inside the editor's dialog one Escape closes only it;
// focus starts on "Repeat every"; and a rule read from Google keeps its own monthly choice.

import { useState } from "react";
import { cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";
import type { RepeatRule } from "../../../lib/types";

vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", depth: "standard" }),
}));

import { Dialog } from "../../ui";
import { CustomRecurrenceDialog } from "./CustomRecurrenceDialog";

// jsdom ships no ResizeObserver, and the end date's picker (escapeClipping) builds one.
beforeAll(() => {
  if (!("ResizeObserver" in globalThis)) {
    (globalThis as unknown as { ResizeObserver: unknown }).ResizeObserver = class {
      observe() {}
      unobserve() {}
      disconnect() {}
    };
  }
});

afterEach(() => {
  cleanup();
  vi.resetAllMocks();
});

const T2 = "2026-10-13"; // a Tuesday

function open(rule: RepeatRule | null = null) {
  const onDone = vi.fn();
  const onClose = vi.fn();
  render(<CustomRecurrenceDialog rule={rule} startDate={T2} onDone={onDone} onClose={onClose} />);
  return { onDone, onClose };
}

const done = () => screen.getByRole<HTMLButtonElement>("button", { name: "Done" });
const interval = () => screen.getByLabelText<HTMLInputElement>("Repeat every");
const unit = () => screen.getByLabelText<HTMLSelectElement>("Unit");
const count = () => screen.getByLabelText<HTMLInputElement>("Occurrences");
const endDate = () => screen.getByLabelText<HTMLInputElement>("End date");
const day = (name: string) => screen.getByRole<HTMLButtonElement>("button", { name });

/** Type `text` into a date field and leave it, as the user would. */
function typeDate(input: HTMLInputElement, text: string) {
  input.focus();
  fireEvent.change(input, { target: { value: text } });
  fireEvent.blur(input);
}

describe("CustomRecurrenceDialog", () => {
  it("focus starts on Repeat every", () => {
    open();
    expect(screen.getByRole("dialog", { name: "Custom recurrence" })).toBeTruthy();
    expect(document.activeElement).toBe(interval());
  });

  it("After 731 is rejected and Done stays disabled", () => {
    const { onDone } = open();
    fireEvent.click(screen.getByLabelText("After"));
    fireEvent.change(count(), { target: { value: "731" } });
    expect(
      screen.getByText("Google allows at most 730 occurrences of a repeating event."),
    ).toBeTruthy();
    expect(done().disabled).toBe(true);
    fireEvent.click(done());
    expect(onDone).not.toHaveBeenCalled();
  });

  it("After 730 is accepted", () => {
    const { onDone } = open();
    // The count is off until its row is chosen.
    expect(count().disabled).toBe(true);
    fireEvent.click(screen.getByLabelText("After"));
    expect(count().disabled).toBe(false);
    fireEvent.change(count(), { target: { value: "730" } });
    fireEvent.click(done());
    expect(onDone).toHaveBeenCalledWith({
      pattern: { kind: "weekly", days: ["tue"] },
      interval: 1,
      ends: { kind: "after", count: 730 },
    });
    // "occurrence" for one.
    fireEvent.change(count(), { target: { value: "1" } });
    expect(screen.getByText("occurrence")).toBeTruthy();
  });

  // The end date has no lower limit: a date before the start is said, not silently moved to it
  // (C11 critiques C1-M5, C2-M3).
  it("an Ends date before the start is rejected", () => {
    const { onDone } = open();
    expect(endDate().disabled).toBe(true);
    fireEvent.click(screen.getByLabelText("On"));
    typeDate(endDate(), "01-10-2026");
    expect(endDate().value).toBe("01-10-2026");
    expect(screen.getByText("The end date is before the first occurrence.")).toBeTruthy();
    expect(done().disabled).toBe(true);
    // On the start day itself, it's fine.
    typeDate(endDate(), "13-10-2026");
    fireEvent.click(done());
    expect(onDone).toHaveBeenCalledWith({
      pattern: { kind: "weekly", days: ["tue"] },
      interval: 1,
      ends: { kind: "on_date", date: T2 },
    });
  });

  // The field shows its date again after text it can't read: Done must not end the rule on it.
  it("an end date it can't read waits until it's given one", () => {
    const { onDone } = open();
    fireEvent.click(screen.getByLabelText("On"));
    typeDate(endDate(), "31-02-2027");
    expect(
      screen.getByText(
        "End date: “31-02-2027” isn't a date PM can read. Type the end date again as dd-mm-yyyy, or pick it from the calendar.",
      ),
    ).toBeTruthy();
    expect(endDate().getAttribute("aria-invalid")).toBe("true");
    expect(done().disabled).toBe(true);
    // Ending another way doesn't need the date.
    fireEvent.click(screen.getByLabelText("Never"));
    expect(done().disabled).toBe(false);
    fireEvent.click(screen.getByLabelText("On"));
    expect(done().disabled).toBe(true);
    typeDate(endDate(), "20-12-2026");
    fireEvent.click(done());
    expect(onDone).toHaveBeenCalledWith(
      expect.objectContaining({ ends: { kind: "on_date", date: "2026-12-20" } }),
    );
  });

  it("weekday toggles are aria-pressed and turning the last one off says to pick one", () => {
    open();
    expect(screen.getByRole("group", { name: "Repeat on" })).toBeTruthy();
    // Monday first; the start's own day is on.
    const names = Array.from(
      screen.getByRole("group", { name: "Repeat on" }).querySelectorAll("button"),
    ).map((b) => [b.textContent, b.getAttribute("aria-label"), b.getAttribute("aria-pressed")]);
    expect(names).toEqual([
      ["M", "Monday", "false"],
      ["T", "Tuesday", "true"],
      ["W", "Wednesday", "false"],
      ["T", "Thursday", "false"],
      ["F", "Friday", "false"],
      ["S", "Saturday", "false"],
      ["S", "Sunday", "false"],
    ]);
    fireEvent.click(day("Tuesday"));
    expect(day("Tuesday").getAttribute("aria-pressed")).toBe("false");
    expect(screen.getByText("Pick at least one day of the week.")).toBeTruthy();
    expect(done().disabled).toBe(true);
    fireEvent.click(day("Friday"));
    fireEvent.click(day("Monday"));
    expect(done().disabled).toBe(false);
  });

  it("the unit's plural follows the interval", () => {
    open();
    const units = () => Array.from(unit().options).map((o) => o.text);
    expect(units()).toEqual(["day", "week", "month", "year"]);
    fireEvent.change(interval(), { target: { value: "2" } });
    expect(units()).toEqual(["days", "weeks", "months", "years"]);
    fireEvent.change(interval(), { target: { value: "100" } });
    expect(screen.getByText("Repeat every 1 to 99.")).toBeTruthy();
    expect(done().disabled).toBe(true);
  });

  it("Done hands back the rule; Cancel hands back nothing", () => {
    const first = open();
    fireEvent.change(interval(), { target: { value: "2" } });
    fireEvent.click(day("Monday"));
    fireEvent.click(done());
    expect(first.onDone).toHaveBeenCalledWith({
      pattern: { kind: "weekly", days: ["mon", "tue"] },
      interval: 2,
      ends: { kind: "never" },
    });
    expect(first.onClose).not.toHaveBeenCalled();
    cleanup();
    const second = open();
    fireEvent.change(unit(), { target: { value: "year" } });
    expect(screen.getByText("Annually on 13 October")).toBeTruthy();
    fireEvent.click(screen.getByRole("button", { name: "Cancel" }));
    expect(second.onClose).toHaveBeenCalledTimes(1);
    expect(second.onDone).not.toHaveBeenCalled();
  });

  it("Escape closes only the custom dialog", () => {
    const outerClose = vi.fn();
    function Harness() {
      const [custom, setCustom] = useState(true);
      return (
        <Dialog open onClose={outerClose} title="New event">
          <p>The editor</p>
          {custom && (
            <CustomRecurrenceDialog
              rule={null}
              startDate={T2}
              onDone={() => setCustom(false)}
              onClose={() => setCustom(false)}
            />
          )}
        </Dialog>
      );
    }
    render(<Harness />);
    expect(screen.getByRole("dialog", { name: "Custom recurrence" })).toBeTruthy();
    fireEvent.keyDown(window, { key: "Escape" });
    expect(screen.queryByRole("dialog", { name: "Custom recurrence" })).toBeNull();
    expect(outerClose).not.toHaveBeenCalled();
    expect(screen.getByRole("dialog", { name: "New event" })).toBeTruthy();
  });

  it("opening on a monthly last-Friday rule keeps that choice", () => {
    const { onDone } = open({
      pattern: { kind: "monthly_weekday", nth: "last", weekday: "fri" },
      interval: 1,
      ends: { kind: "never" },
    });
    const monthly = screen.getByLabelText<HTMLSelectElement>("Monthly on");
    expect(Array.from(monthly.options).map((o) => o.text)).toEqual([
      "Monthly on day 13",
      "Monthly on the second Tuesday",
      "Monthly on the last Friday",
    ]);
    expect(monthly.selectedOptions[0].text).toBe("Monthly on the last Friday");
    // Picking another and back keeps it on offer.
    fireEvent.change(monthly, { target: { value: "0" } });
    fireEvent.change(monthly, { target: { value: "2" } });
    fireEvent.click(done());
    expect(onDone).toHaveBeenCalledWith({
      pattern: { kind: "monthly_weekday", nth: "last", weekday: "fri" },
      interval: 1,
      ends: { kind: "never" },
    });
  });
});
