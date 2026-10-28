// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// An event's notifications as the editor reads and writes them (#884, C13): timed minutes in the
// largest whole unit, all-day minutes as days or weeks before a time of day (counted from the
// midnight before the event, INFERRED E9b), the words for each, the rows PM refuses, and lists that
// compare in any order, as the backend's do.

import { describe, expect, it } from "vitest";
import {
  allDayMinutes,
  allDayParts,
  calendarDefaults,
  copyReminders,
  defaultsButtonLabel,
  isEditable,
  kindSwitchText,
  newRow,
  parseCount,
  reminderText,
  rowProblem,
  sameReminders,
  timedMinutes,
  timedParts,
} from "./reminders";

const own = (...list: [string, number][]) => ({
  use_default: false,
  overrides: list.map(([method, minutes]) => ({ method, minutes })),
});

describe("notifications", () => {
  it("timed minutes read in the largest whole unit", () => {
    expect(timedParts(90)).toEqual({ n: 90, unit: "minutes" });
    expect(timedParts(120)).toEqual({ n: 2, unit: "hours" });
    expect(timedParts(1440)).toEqual({ n: 1, unit: "days" });
    expect(timedParts(10080)).toEqual({ n: 1, unit: "weeks" });
    expect(timedParts(0)).toEqual({ n: 0, unit: "minutes" });
    for (const m of [0, 5, 90, 120, 1500, 2880, 10080, 40320]) {
      expect(timedMinutes(timedParts(m))).toBe(m);
    }
  });

  it("all-day 1 day before at 09:00 is 900", () => {
    expect(allDayMinutes({ n: 1, unit: "days", at: "09:00" })).toBe(900);
    expect(allDayMinutes({ n: 1, unit: "weeks", at: "09:00" })).toBe(9540);
    expect(allDayMinutes({ n: 4, unit: "weeks", at: "00:00" })).toBe(40320);
  });

  it("all-day values read back as days or weeks and a time", () => {
    expect(allDayParts(900)).toEqual({ n: 1, unit: "days", at: "09:00" });
    expect(allDayParts(1440)).toEqual({ n: 1, unit: "days", at: "00:00" });
    expect(allDayParts(9540)).toEqual({ n: 1, unit: "weeks", at: "09:00" });
    expect(allDayParts(10080)).toEqual({ n: 1, unit: "weeks", at: "00:00" });
    expect(allDayParts(0)).toEqual({ n: 0, unit: "days", at: "00:00" });
    expect(allDayParts(2340)).toEqual({ n: 2, unit: "days", at: "09:00" });
    for (const m of [15, 900, 1440, 2340, 9540, 10080, 39780]) {
      expect(allDayMinutes(allDayParts(m))).toBe(m);
    }
  });

  it("a value Google holds outside the range still reads", () => {
    // On the day itself, which Google's range doesn't cover (INFERRED, E9d).
    expect(allDayParts(-540)).toEqual({ n: 0, unit: "days", at: "09:00" });
    expect(allDayParts(40321)).toEqual({ n: 29, unit: "days", at: "23:59" });
    expect(timedParts(40321)).toEqual({ n: 40321, unit: "minutes" });
    expect(reminderText({ method: "popup", minutes: -540 }, true)).toBe("On the day at 09:00");
    expect(reminderText({ method: "popup", minutes: -10 }, false)).toBe(
      "10 minutes after it starts",
    );
  });

  it("rowProblem refuses past four weeks, a fraction, and 0 days for all-day", () => {
    expect(rowProblem(40320, false)).toBeNull();
    expect(rowProblem(40321, false)).toBe("Google allows notifications up to 4 weeks before.");
    expect(rowProblem(40321, true)).toBe("Google allows notifications up to 4 weeks before.");
    expect(rowProblem(null, false)).toBe("Use a whole number.");
    expect(rowProblem(1.5, false)).toBe("Use a whole number.");
    expect(rowProblem(-1, false)).toBe("Use a whole number.");
    expect(rowProblem(0, false)).toBeNull();
    expect(rowProblem(0, true)).toBe("Pick at least 1 day before.");
    // 0 days at 09:00 is a whole number typed, on the day itself (C13 review round 1).
    expect(rowProblem(-540, true)).toBe("Pick at least 1 day before.");
    expect(rowProblem(null, true)).toBe("Use a whole number.");
    expect(rowProblem(15, true)).toBeNull();
    // What a row's box can hold.
    expect(parseCount(" 12 ")).toBe(12);
    for (const text of ["", "1.5", "-3", "ten", "1e3"]) expect(parseCount(text)).toBeNull();
  });

  it("only a way PM sends, in Google's range, shows as fields", () => {
    expect(isEditable({ method: "popup", minutes: 30 }, false)).toBe(true);
    expect(isEditable({ method: "email", minutes: 900 }, true)).toBe(true);
    expect(isEditable({ method: "sms", minutes: 30 }, false)).toBe(false);
    expect(isEditable({ method: "popup", minutes: 40321 }, false)).toBe(false);
    expect(isEditable({ method: "popup", minutes: -540 }, true)).toBe(false);
  });

  it("reminderText words", () => {
    const timed = (minutes: number, method = "popup") => reminderText({ method, minutes }, false);
    expect(timed(0)).toBe("When it starts");
    expect(timed(30)).toBe("30 minutes before");
    expect(timed(1)).toBe("1 minute before");
    expect(timed(60)).toBe("1 hour before");
    expect(timed(90)).toBe("90 minutes before");
    expect(timed(1440)).toBe("1 day before");
    expect(timed(10080)).toBe("1 week before");
    expect(timed(60, "email")).toBe("1 hour before, by email");
    expect(timed(30, "sms")).toBe("30 minutes before (sent a way PM can't change)");
    const allDay = (minutes: number) => reminderText({ method: "popup", minutes }, true);
    expect(allDay(900)).toBe("1 day before at 09:00");
    expect(allDay(1440)).toBe("1 day before at 00:00");
    expect(allDay(9540)).toBe("1 week before at 09:00");
    expect(allDay(2340)).toBe("2 days before at 09:00");
    expect(allDay(-540)).toBe("On the day at 09:00");
  });

  it("sameReminders ignores order", () => {
    expect(
      sameReminders(own(["popup", 10], ["email", 60]), own(["email", 60], ["popup", 10])),
    ).toBe(true);
    expect(sameReminders(own(["popup", 10]), own(["popup", 15]))).toBe(false);
    // One more on either side is a change.
    expect(sameReminders(own(["email", 5]), own(["email", 5], ["popup", 10]))).toBe(false);
    expect(sameReminders(own(["popup", 10]), own(["email", 10]))).toBe(false);
    // Counted, not just seen: one twice isn't the same as it and another.
    expect(sameReminders(own(["popup", 10], ["popup", 10]), own(["popup", 10], ["email", 5]))).toBe(
      false,
    );
    // The defaults are the defaults; an empty list of the event's own is not them.
    expect(sameReminders(calendarDefaults(), { use_default: true, overrides: [] })).toBe(true);
    expect(sameReminders(calendarDefaults(), own())).toBe(false);
    expect(sameReminders(own(), own())).toBe(true);
  });

  it("each value is its own: the defaults are made fresh, and a copy shares nothing", () => {
    const a = calendarDefaults();
    a.overrides.push({ method: "popup", minutes: 5 });
    expect(calendarDefaults()).toEqual({ use_default: true, overrides: [] });
    const list = own(["popup", 10]);
    const copy = copyReminders(list);
    copy.overrides[0].minutes = 20;
    expect(list.overrides[0].minutes).toBe(10);
  });

  it("a new row is Google's, or the next one free", () => {
    expect(newRow([], false)).toEqual({ method: "popup", minutes: 30 });
    expect(newRow([{ method: "popup", minutes: 30 }], false)).toEqual({
      method: "popup",
      minutes: 10,
    });
    expect(newRow([{ method: "email", minutes: 30 }], false)).toEqual({
      method: "popup",
      minutes: 30,
    });
    expect(newRow([], true)).toEqual({ method: "popup", minutes: 900 });
    expect(newRow([{ method: "popup", minutes: 900 }], true)).toEqual({
      method: "popup",
      minutes: 2340,
    });
  });

  it("names the calendar, or says the calendar's", () => {
    expect(defaultsButtonLabel("Work")).toBe("Use Work's defaults");
    expect(defaultsButtonLabel(null)).toBe("Use the calendar's defaults");
    expect(kindSwitchText("Work", true)).toBe(
      "Notifications went back to Work's defaults for all-day events.",
    );
    expect(kindSwitchText(null, false)).toBe(
      "Notifications went back to the calendar's defaults for timed events.",
    );
  });
});
