// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { afterEach, describe, expect, it, vi } from "vitest";

import { deviceTimeZone, deviceTimeZoneOrNull } from "./timezones";

describe("the device zone", () => {
  afterEach(() => {
    vi.restoreAllMocks();
  });

  /** Make the runtime report `zone` (or throw) as the device's zone. */
  function reports(zone: string | undefined | Error) {
    vi.spyOn(Intl, "DateTimeFormat").mockImplementation(() => {
      if (zone instanceof Error) throw zone;
      return { resolvedOptions: () => ({ timeZone: zone }) } as unknown as Intl.DateTimeFormat;
    });
  }

  it("is the runtime's zone when it reports one", () => {
    reports("Europe/London");
    expect(deviceTimeZoneOrNull()).toBe("Europe/London");
    expect(deviceTimeZone()).toBe("Europe/London");
  });

  it("is unknown, not UTC, for anything that writes a time", () => {
    for (const broken of [undefined, "", new Error("no Intl")]) {
      reports(broken);
      expect(deviceTimeZoneOrNull()).toBeNull();
      // The display-only reading keeps its old fallback.
      expect(deviceTimeZone()).toBe("UTC");
    }
  });
});
