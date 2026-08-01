// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Documents tab unmounts on every tab switch, so anything here that is a standing preference
// has to survive that. These pin the tri-state fold in particular: `null` (never chosen) must stay
// distinguishable from `false` (deliberately closed), or the caller cannot own the default.

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  ACTIVITY_OPEN_KEY,
  readActivityOpen,
  writeActivityOpen,
  readCopyPhotosToVault,
  writeCopyPhotosToVault,
} from "./documentPrefs";

beforeEach(() => localStorage.clear());
afterEach(() => vi.restoreAllMocks());

describe("the Activity fold survives the tab switch that unmounts Documents", () => {
  it("reports null when the user has never chosen", () => {
    expect(readActivityOpen()).toBeNull();
  });

  it("round-trips both states, and closed is NOT the same as unset", () => {
    writeActivityOpen(false);
    expect(readActivityOpen()).toBe(false);
    expect(localStorage.getItem(ACTIVITY_OPEN_KEY)).toBe("false");
    writeActivityOpen(true);
    expect(readActivityOpen()).toBe(true);
  });

  it("treats a junk value as unset rather than closed", () => {
    localStorage.setItem(ACTIVITY_OPEN_KEY, "yes");
    // Not `false`: defaulting a junk read to closed would silently hide the Activity list, which
    // is the one thing on this card the user came to see.
    expect(readActivityOpen()).toBeNull();
  });

  it("survives a localStorage that throws (private mode / quota)", () => {
    vi.spyOn(Storage.prototype, "getItem").mockImplementation(() => {
      throw new Error("denied");
    });
    expect(readActivityOpen()).toBeNull();
    vi.spyOn(Storage.prototype, "setItem").mockImplementation(() => {
      throw new Error("denied");
    });
    expect(() => writeActivityOpen(true)).not.toThrow();
  });
});

describe("copy-photos stays a standing preference", () => {
  it("defaults off and round-trips", () => {
    expect(readCopyPhotosToVault()).toBe(false);
    writeCopyPhotosToVault(true);
    expect(readCopyPhotosToVault()).toBe(true);
  });
});
