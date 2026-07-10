// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { describe, it, expect } from "vitest";
import { guideFor, type SetupPlatform } from "./setupGuide";

// The guide copy is per-platform (the OS detection itself is a UA regex exercised in
// the app); these pin the routing so a copy edit can't silently hand Linux users the
// Windows python.org instructions again.

const PLATFORMS: SetupPlatform[] = ["windows", "mac", "linux"];

describe("guideFor", () => {
  it("gives every platform a non-empty guide for every mode", () => {
    const modes = [
      "install",
      "python_missing",
      "python_too_old",
      "python_download_failed",
      "pip_failed",
      "requirements_missing",
      "packaging_bug",
      "unknown",
    ] as const;
    for (const platform of PLATFORMS) {
      for (const mode of modes) {
        const g = guideFor(mode, platform);
        expect(g.title.length, `${platform}/${mode} title`).toBeGreaterThan(0);
        expect(g.steps.length, `${platform}/${mode} steps`).toBeGreaterThan(0);
      }
    }
  });

  it("routes python_missing to the platform's own package story", () => {
    expect(guideFor("python_missing", "linux").steps[0]).toContain("dnf");
    expect(guideFor("python_missing", "windows").steps[0]).toContain("python.org");
    expect(guideFor("python_missing", "mac").steps[0]).toContain("brew");
  });

  it("never tells a Linux or Mac user about python.exe", () => {
    for (const platform of ["linux", "mac"] as const) {
      const g = guideFor("python_missing", platform);
      expect(JSON.stringify(g)).not.toContain("python.exe");
    }
  });

  it("describes the self-contained bundled setup on windows and linux alike", () => {
    expect(guideFor("install", "linux")).toEqual(guideFor("install", "windows"));
  });
});
