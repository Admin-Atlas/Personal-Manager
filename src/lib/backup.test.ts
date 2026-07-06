// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import { isOpaquePhase, describeFailures } from "./backup";

describe("isOpaquePhase — shimmer vs percent bar (F-45)", () => {
  it("treats upload and download as opaque (they have no honest byte fraction)", () => {
    expect(isOpaquePhase("upload")).toBe(true);
    expect(isOpaquePhase("download")).toBe(true);
  });

  it("leaves the metered phases as determinate", () => {
    // snapshot/pack (byte-metered via ProgressReader) and restore/validate report real fractions.
    expect(isOpaquePhase("snapshot")).toBe(false);
    expect(isOpaquePhase("pack")).toBe(false);
    expect(isOpaquePhase("restore")).toBe(false);
    expect(isOpaquePhase("validate")).toBe(false);
  });

  it("is safe on a null phase (no op in flight)", () => {
    expect(isOpaquePhase(null)).toBe(false);
  });
});

describe("describeFailures — partial-failure banner copy (F-22)", () => {
  it("returns null on a clean run so the banner stays hidden", () => {
    expect(describeFailures([])).toBeNull();
  });

  it("uses the singular noun for one failed destination", () => {
    const msg = describeFailures(["Google Drive: 401 unauthorized"]);
    expect(msg).toContain("1 destination failed");
    expect(msg).not.toContain("destinations failed");
    expect(msg).toContain("Google Drive: 401 unauthorized");
  });

  it("uses the plural noun and joins multiple failures", () => {
    const msg = describeFailures(["Proton Drive: timed out", "Google Drive: quota"]);
    expect(msg).toContain("2 destinations failed");
    expect(msg).toContain("Proton Drive: timed out; Google Drive: quota");
  });

  it("reassures that the successful destinations still got the archive", () => {
    // The banner is non-blocking: at least one destination succeeded (that's the only time the
    // backend populates failed_destinations), so the copy must not read as a total failure.
    expect(describeFailures(["X: nope"])).toContain("did reach the destinations that succeeded");
  });
});
