// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, expect, it } from "vitest";
import { editingTurnedOff } from "./calendarEditing";

describe("editingTurnedOff", () => {
  it("lists the accounts a read-only connect turned off", () => {
    expect(
      editingTurnedOff(
        {
          "gcal:a@example.com": "on",
          "gcal:b@example.com": "paused",
          "gcal:c@example.com": "needs_consent",
          "gcal:d@example.com": "off",
          "gcal:e@example.com": "on",
        },
        {
          "gcal:a@example.com": "off",
          "gcal:b@example.com": "off",
          "gcal:c@example.com": "off",
          "gcal:d@example.com": "off",
          "gcal:e@example.com": "on",
        },
      ),
    ).toEqual(["gcal:a@example.com", "gcal:b@example.com", "gcal:c@example.com"]);
  });

  it("doesn't count an account that went away or was never on", () => {
    expect(editingTurnedOff({ "gcal:a@example.com": "on" }, {})).toEqual([]);
    expect(editingTurnedOff({}, { "gcal:new@example.com": "off" })).toEqual([]);
  });
});
