// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import { isOpaquePhase, describeFailures, describeForgetConsequences } from "./backup";

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

describe("describeForgetConsequences — the SILENT half of forgetting the passphrase", () => {
  it("says nothing when there is no schedule to lose", () => {
    // A false alarm is its own defect: someone who never turned automatic backups on must not be
    // warned that this turns them off.
    expect(describeForgetConsequences("off")).toBeNull();
  });

  it("names the user's own cadence, and that it becomes Off", () => {
    for (const [freq, label] of [
      ["daily", "Daily"],
      ["weekly", "Weekly"],
      ["monthly", "Monthly"],
    ] as const) {
      const msg = describeForgetConsequences(freq);
      expect(msg).toContain(label);
      expect(msg).toContain("switches them to Off");
    }
  });

  it("says what SURVIVES, so the warning isn't read as 'this deletes my backups'", () => {
    // `forget_backup_passphrase` touches the cadence and the keychain and nothing else — the
    // destinations, the retention count and every archive are untouched. The dialog that carries
    // this sentence also carries the unreadable-backups claim, so under-stating what survives
    // would make it read as a delete.
    const msg = describeForgetConsequences("weekly") ?? "";
    expect(msg).toContain("untouched");
    expect(msg).toContain("how many backups to keep");
  });
});
