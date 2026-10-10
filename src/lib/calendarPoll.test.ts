// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { CHECK_EVERY_MS, nextCheckDelay, shouldCheck } from "./calendarPoll";

describe("the calendar's change check cadence", () => {
  it("aims for every 30 seconds, give or take a quarter", () => {
    expect(CHECK_EVERY_MS).toBe(30_000);
    expect(nextCheckDelay(0)).toBe(22_500);
    expect(nextCheckDelay(0.5)).toBe(30_000);
    expect(nextCheckDelay(0.999_999)).toBeLessThanOrEqual(37_500);
    for (let i = 0; i < 100; i++) {
      const d = nextCheckDelay();
      expect(d).toBeGreaterThanOrEqual(22_500);
      expect(d).toBeLessThanOrEqual(37_500);
    }
  });

  it("checks only while PM is on screen", () => {
    expect(shouldCheck("visible")).toBe(true);
    expect(shouldCheck("hidden")).toBe(false);
  });
});
