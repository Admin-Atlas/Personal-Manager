// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// @vitest-environment jsdom
//
// The editor's Notifications (#884, C13): a timed event's defaults as rows that become its own on
// the first change, an all-day event's named with Change, at most five (Add says why, and stays
// focusable), a row that can't be committed refused under it and reported to the editor, one Google
// holds that PM can't make kept as a line, back to the defaults, focus after every change, and every
// control named by its row.

import { useState } from "react";
import { act, cleanup, fireEvent, render, screen, within } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { Reminder, RemindersDraft } from "../../../lib/types";

vi.mock("../../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", depth: "standard" }),
}));

import { ReminderList } from "./ReminderList";

const own = (...list: [string, number][]): RemindersDraft => ({
  use_default: false,
  overrides: list.map(([method, minutes]) => ({ method, minutes })),
});
const DEFAULTS: RemindersDraft = { use_default: true, overrides: [] };

/** The list as the editor holds it: every change kept, every one and every refusal recorded. */
function harness(
  start: RemindersDraft,
  opts: { allDay?: boolean; defaults?: Reminder[]; disabled?: boolean; unreadable?: boolean } = {},
) {
  const changes: RemindersDraft[] = [];
  const invalid: boolean[] = [];
  function Host() {
    const [value, setValue] = useState(start);
    return (
      <ReminderList
        value={value}
        allDay={opts.allDay ?? false}
        calendar={{ name: "Work", default_reminders: opts.defaults ?? [] }}
        disabled={opts.disabled ?? false}
        unreadable={opts.unreadable}
        onChange={(next) => {
          changes.push(next);
          setValue(next);
        }}
        onInvalid={(bad) => invalid.push(bad)}
      />
    );
  }
  const view = render(<Host />);
  return { changes, invalid, unmount: view.unmount };
}

const count = (words: string) =>
  screen.getByRole<HTMLInputElement>("textbox", { name: `How many: ${words}` });
const add = () => screen.getByRole("button", { name: "Add notification" });
const last = <T,>(list: readonly T[]): T | undefined => list[list.length - 1];

/** Type `text` in a row's number and leave it. */
function retype(input: HTMLInputElement, text: string) {
  input.focus();
  fireEvent.change(input, { target: { value: text } });
  fireEvent.blur(input);
}

afterEach(() => {
  cleanup();
});

describe("ReminderList", () => {
  it("shows the calendar's defaults as rows, and the first change makes them the event's own", () => {
    const { changes } = harness(DEFAULTS, {
      defaults: [
        { method: "popup", minutes: 10 },
        { method: "email", minutes: 60 },
      ],
    });
    expect(screen.getByText("Work's default notifications:")).toBeTruthy();
    expect(count("10 minutes before").value).toBe("10");
    expect(count("1 hour before, by email").value).toBe("1");
    expect(screen.queryByRole("button", { name: "Use Work's defaults" })).toBeNull();
    retype(count("10 minutes before"), "15");
    expect(changes).toEqual([own(["popup", 15], ["email", 60])]);
    expect(screen.queryByText("Work's default notifications:")).toBeNull();
    expect(screen.getByRole("button", { name: "Use Work's defaults" })).toBeTruthy();
  });

  it("says when the calendar has no defaults, and when the event has none of its own", () => {
    harness(DEFAULTS);
    expect(screen.getByText("Work has no default notifications.")).toBeTruthy();
    cleanup();
    harness(own());
    expect(screen.getByText("No notifications.")).toBeTruthy();
  });

  it("an all-day event on the defaults says so without a list; Change starts one", async () => {
    const { changes } = harness(DEFAULTS, {
      allDay: true,
      defaults: [{ method: "popup", minutes: 30 }],
    });
    expect(screen.getByText("Work's default notifications for all-day events.")).toBeTruthy();
    // The timed defaults aren't an all-day event's.
    expect(screen.queryByRole("textbox")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Change" }));
    expect(changes).toEqual([own(["popup", 900])]);
    const row = count("1 day before at 09:00");
    await act(async () => {});
    expect(document.activeElement).toBe(row);
    // Days or weeks, before a time of day.
    expect(screen.getByRole("combobox", { name: "Unit: 1 day before at 09:00" })).toBeTruthy();
    fireEvent.change(screen.getByRole("combobox", { name: "At: 1 day before at 09:00" }), {
      target: { value: "18:00" },
    });
    expect(last(changes)).toEqual(own(["popup", 360]));
    fireEvent.change(screen.getByRole("combobox", { name: "Unit: 1 day before at 18:00" }), {
      target: { value: "weeks" },
    });
    expect(last(changes)).toEqual(own(["popup", 10080 - 1080]));
  });

  it("Add is off at five, stays focusable, and says why", () => {
    const five = own(["popup", 5], ["popup", 10], ["popup", 15], ["popup", 20], ["popup", 25]);
    const { changes } = harness(five);
    const button = add();
    expect(button.getAttribute("aria-disabled")).toBe("true");
    expect((button as HTMLButtonElement).disabled).toBe(false);
    const why = screen.getByText("Google allows up to 5 notifications.");
    expect(button.getAttribute("aria-describedby")).toBe(why.id);
    fireEvent.click(button);
    expect(changes).toEqual([]);
  });

  it("a row past four weeks shows its problem, isn't committed, and holds the editor's save", () => {
    const { changes, invalid } = harness(own(["popup", 10]));
    const row = count("10 minutes before");
    fireEvent.change(screen.getByRole("combobox", { name: "Unit: 10 minutes before" }), {
      target: { value: "weeks" },
    });
    expect(screen.getByText("Google allows notifications up to 4 weeks before.")).toBeTruthy();
    expect(row.getAttribute("aria-invalid")).toBe("true");
    expect(changes).toEqual([]);
    expect(invalid).toEqual([true]);
    // Fixed, it commits, and the editor may save again.
    retype(row, "2");
    expect(changes).toEqual([own(["popup", 20160])]);
    expect(invalid).toEqual([true, false]);
    expect(screen.queryByText(/up to 4 weeks/)).toBeNull();
  });

  it("refuses a fraction, and the same notification twice", () => {
    const { changes, invalid } = harness(own(["popup", 10], ["popup", 30]));
    retype(count("10 minutes before"), "1.5");
    expect(screen.getByText("Use a whole number.")).toBeTruthy();
    retype(count("10 minutes before"), "30");
    expect(screen.getByText("Already on the list.")).toBeTruthy();
    expect(changes).toEqual([]);
    expect(invalid).toEqual([true]);
  });

  // C13 review round 1: 0 days at 09:00 is a whole number on the day itself, which an all-day
  // event's count from the midnight before it can't hold: said as that, at any time of day.
  it("an all-day row of 0 days says to pick at least 1 day, whatever its time", () => {
    const { changes, invalid } = harness(own(["popup", 900]), { allDay: true });
    const row = count("1 day before at 09:00");
    retype(row, "0");
    expect(screen.getByText("Pick at least 1 day before.")).toBeTruthy();
    expect(screen.queryByText("Use a whole number.")).toBeNull();
    expect(row.getAttribute("aria-invalid")).toBe("true");
    expect(changes).toEqual([]);
    expect(invalid).toEqual([true]);
  });

  it("a removed row that couldn't be committed no longer holds the save", () => {
    const { invalid } = harness(own(["popup", 10], ["popup", 30]));
    retype(count("10 minutes before"), "ten");
    expect(invalid).toEqual([true]);
    fireEvent.click(screen.getByRole("button", { name: "Remove notification: 10 minutes before" }));
    expect(invalid).toEqual([true, false]);
  });

  it("an old method Google holds is shown, kept, and removable", () => {
    const { changes } = harness(own(["sms", 30], ["popup", 10]));
    const line = screen.getByText("30 minutes before (sent a way PM can't change)");
    expect(within(line.closest("li")!).queryByRole("textbox")).toBeNull();
    // Another row's change keeps it as Google holds it.
    retype(count("10 minutes before"), "20");
    expect(last(changes)).toEqual(own(["sms", 30], ["popup", 20]));
    fireEvent.click(
      screen.getByRole("button", {
        name: "Remove notification: 30 minutes before (sent a way PM can't change)",
      }),
    );
    expect(last(changes)).toEqual(own(["popup", 20]));
  });

  it("Use the defaults puts use_default back, focus on Add (timed) or Change (all day)", async () => {
    const { changes } = harness(own(["popup", 10]));
    fireEvent.click(screen.getByRole("button", { name: "Use Work's defaults" }));
    expect(changes).toEqual([DEFAULTS]);
    await act(async () => {});
    expect(document.activeElement).toBe(add());
    cleanup();
    harness(own(["popup", 900]), { allDay: true });
    fireEvent.click(screen.getByRole("button", { name: "Use Work's defaults" }));
    await act(async () => {});
    expect(document.activeElement).toBe(screen.getByRole("button", { name: "Change" }));
  });

  it("Add goes to the new row's number, Remove to the row before or to Add", async () => {
    harness(own(["popup", 10]));
    fireEvent.click(add());
    await act(async () => {});
    expect(document.activeElement).toBe(count("30 minutes before"));
    fireEvent.click(screen.getByRole("button", { name: "Remove notification: 30 minutes before" }));
    await act(async () => {});
    expect(document.activeElement).toBe(
      screen.getByRole("button", { name: "Remove notification: 10 minutes before" }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Remove notification: 10 minutes before" }));
    await act(async () => {});
    expect(document.activeElement).toBe(add());
    expect(screen.getByText("No notifications.")).toBeTruthy();
  });

  it("every control is named, by its row", () => {
    harness(own(["popup", 10], ["email", 60]));
    for (const words of ["10 minutes before", "1 hour before, by email"]) {
      expect(screen.getByRole("combobox", { name: `How: ${words}` })).toBeTruthy();
      expect(screen.getByRole("textbox", { name: `How many: ${words}` })).toBeTruthy();
      expect(screen.getByRole("combobox", { name: `Unit: ${words}` })).toBeTruthy();
      expect(screen.getByRole("button", { name: `Remove notification: ${words}` })).toBeTruthy();
    }
    expect(screen.getByRole("group", { name: "Notifications" })).toBeTruthy();
    expect(screen.getByText("Google Calendar sends these, not PM.")).toBeTruthy();
    // Singular at one.
    const unit = screen.getByRole<HTMLSelectElement>("combobox", {
      name: "Unit: 1 hour before, by email",
    });
    expect(unit.selectedOptions[0].textContent).toBe("hour");
  });

  it("off, nothing changes; unreadable, nothing is shown as the event's", () => {
    harness(own(["popup", 10]), { disabled: true });
    expect(count("10 minutes before").disabled).toBe(true);
    expect((add() as HTMLButtonElement).disabled).toBe(true);
    cleanup();
    harness(DEFAULTS, { unreadable: true, defaults: [{ method: "popup", minutes: 10 }] });
    expect(screen.queryByRole("textbox")).toBeNull();
    expect(screen.queryByText(/default notifications/)).toBeNull();
    expect(screen.queryByRole("button", { name: "Add notification" })).toBeNull();
    expect(screen.getByText("Google Calendar sends these, not PM.")).toBeTruthy();
  });
});
