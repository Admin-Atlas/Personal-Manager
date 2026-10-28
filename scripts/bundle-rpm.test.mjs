// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The rpm's version label. The release and the Linux dry-run are the only places the bundling
// itself runs, so what's pinned here is the part they depend on: every label this produces is
// legal for both Tauri (semver) and rpm, PM's real version labels cleanly today, the renaming
// back touches only the rpm that was just built, and arguments that would make Tauri write the
// rpm somewhere else are refused.
//
// Importing the module does not run the bundler (entry-point guard at the bottom of it).

import { readFileSync } from "node:fs";
import { describe, expect, it } from "vitest";

import { movingArg, renamesFor, rpmVersion } from "./bundle-rpm.mjs";

// The characters rpm-rs 0.30 allows in a Version tag, quoted from its error message.
const RPM_LEGAL = /^[A-Za-z0-9._+%{}~^]+$/;
// semver 2.0.0 with optional pre-release and build metadata (no leading zeros checked here).
const SEMVER =
  /^\d+\.\d+\.\d+(-[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?(\+[0-9A-Za-z-]+(\.[0-9A-Za-z-]+)*)?$/;

describe("rpmVersion", () => {
  it("turns the pre-release hyphen into a plus", () => {
    expect(rpmVersion("3.155.1-alpha")).toBe("3.155.1+alpha");
    expect(rpmVersion("3.1.0-alpha.2")).toBe("3.1.0+alpha.2");
  });

  it("leaves a release version alone", () => {
    expect(rpmVersion("4.0.0")).toBe("4.0.0");
  });

  it("produces labels that are both semver and rpm-legal", () => {
    for (const v of ["3.155.1-alpha", "3.1.0-alpha.2", "4.0.0", "10.20.30-rc.1"]) {
      const label = rpmVersion(v);
      expect(label).toMatch(RPM_LEGAL);
      expect(label).toMatch(SEMVER);
    }
  });

  it("refuses versions it can't label cleanly", () => {
    for (const v of [
      "3.1.0-alpha-1",
      "3.1.0-alpha+b1",
      "3.1.0+b1",
      "v3.1.0",
      "3.1",
      "",
      "3.1.0-",
    ]) {
      expect(() => rpmVersion(v)).toThrow(/can't label version/);
    }
  });

  it("labels PM's current version", () => {
    const conf = JSON.parse(
      readFileSync(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"),
    );
    const label = rpmVersion(conf.version);
    expect(label).toMatch(RPM_LEGAL);
    expect(label).toMatch(SEMVER);
  });
});

describe("renamesFor", () => {
  it("gives the new rpm the release's own file name", () => {
    expect(renamesFor(["PM-3.155.1+alpha-1.x86_64.rpm"], "3.155.1-alpha", "3.155.1+alpha")).toEqual(
      [["PM-3.155.1+alpha-1.x86_64.rpm", "PM-3.155.1-alpha-1.x86_64.rpm"]],
    );
  });

  it("leaves other files alone", () => {
    const files = [
      "PM-3.155.1+alpha-1.x86_64.rpm",
      "PM-3.155.0-alpha-1.x86_64.rpm",
      "PM-3.155.1+alpha-1.x86_64.rpm.sig",
      "notes.txt",
    ];
    expect(renamesFor(files, "3.155.1-alpha", "3.155.1+alpha")).toEqual([
      ["PM-3.155.1+alpha-1.x86_64.rpm", "PM-3.155.1-alpha-1.x86_64.rpm"],
    ]);
  });

  it("renames nothing when the label is the version", () => {
    expect(renamesFor(["PM-4.0.0-1.x86_64.rpm"], "4.0.0", "4.0.0")).toEqual([]);
  });
});

describe("movingArg", () => {
  it("lets output-neutral arguments through", () => {
    expect(movingArg([])).toBeNull();
    expect(movingArg(["--verbose", "-v"])).toBeNull();
  });

  it("names an argument that would move the rpm out of target/release", () => {
    expect(movingArg(["--verbose", "--target", "aarch64-unknown-linux-gnu"])).toBe("--target");
    expect(movingArg(["--target=aarch64-unknown-linux-gnu"])).toBe(
      "--target=aarch64-unknown-linux-gnu",
    );
    expect(movingArg(["-t", "x"])).toBe("-t");
    expect(movingArg(["--debug"])).toBe("--debug");
    expect(movingArg(["-d"])).toBe("-d");
  });
});
