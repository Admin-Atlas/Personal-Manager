// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The rpm's version label. The release and the Linux dry-run are the only places the bundling
// itself runs, so what's pinned here is the part they depend on: every label this produces is
// legal for both Tauri (semver) and rpm, PM's real version labels cleanly today, the renaming
// back touches only the rpm that was just built, an rpm this run didn't write never passes as
// built, and arguments that would make Tauri write the rpm somewhere else are refused.
//
// Importing the module does not run the bundler (entry-point guard at the bottom of it).

import { mkdirSync, mkdtempSync, readFileSync, rmSync, utimesSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

import { movingArg, pickBuilt, renamesFor, rpmVersion, snapshot } from "./bundle-rpm.mjs";

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
    expect(movingArg(["--verbose", "-v", "-vv"])).toBeNull();
    // A value after a value-taking short flag is that flag's, not more flags.
    expect(movingArg(["-fdummy"])).toBeNull();
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

  it("finds them inside clusters of short flags, as clap accepts them", () => {
    for (const a of ["-vd", "-dv", "-vvd", "-dt", "-vt", "-tx86_64-unknown-linux-gnu", "-t=x"]) {
      expect(movingArg([a])).toBe(a);
    }
  });
});

describe("pickBuilt", () => {
  const V = "3.155.1-alpha";
  const L = "3.155.1+alpha";
  const labelled = "PM-3.155.1+alpha-1.x86_64.rpm";
  const named = "PM-3.155.1-alpha-1.x86_64.rpm";

  it("renames the rpm this run wrote under its label", () => {
    const before = [{ name: "PM-3.155.0-alpha-1.x86_64.rpm", sig: "1:10:5" }];
    const after = [...before, { name: labelled, sig: "2:20:9" }];
    expect(pickBuilt(before, after, V, L)).toEqual({
      renames: [[labelled, named]],
      built: named,
    });
  });

  it("takes the new rpm, not the last run's renamed one beside it", () => {
    // Two runs back to back: the first's rpm is already under the release name.
    const before = [{ name: named, sig: "3:30:9" }];
    const after = [...before, { name: labelled, sig: "4:30:10" }];
    expect(pickBuilt(before, after, V, L)).toEqual({ renames: [[labelled, named]], built: named });
  });

  it("counts a labelled rpm the pass rewrote in place", () => {
    const before = [{ name: labelled, sig: "5:30:9" }];
    expect(pickBuilt(before, [{ name: labelled, sig: "5:30:12" }], V, L).built).toBe(named);
  });

  it("goes by the label's name, even when something else in the folder changed too", () => {
    const before = [{ name: named, sig: "3:30:9" }];
    const after = [
      { name: named, sig: "3:30:11" },
      { name: labelled, sig: "4:30:10" },
      { name: "PM-3.155.1+alpha-1.x86_64.rpm.sig", sig: "9:1:10" },
    ];
    expect(pickBuilt(before, after, V, L)).toEqual({ renames: [[labelled, named]], built: named });
  });

  it("refuses two new labelled rpms rather than pick one", () => {
    const after = [
      { name: labelled, sig: "4:30:10" },
      { name: "PM-3.155.1+alpha-1.aarch64.rpm", sig: "5:30:10" },
    ];
    expect(() => pickBuilt([], after, V, L)).toThrow(/found 2/);
  });

  it("refuses when the pass wrote nothing here, whatever is lying around", () => {
    for (const files of [
      [],
      [{ name: named, sig: "6:30:99999" }],
      [{ name: labelled, sig: "7:1:1" }],
    ]) {
      expect(() => pickBuilt(files, files, V, L)).toThrow(
        /expected the pass to write one rpm for 3\.155\.1\+alpha, found 0/,
      );
    }
  });

  it("takes a release version's rpm as it is, if this run wrote it", () => {
    const rpm = "PM-4.0.0-1.x86_64.rpm";
    expect(pickBuilt([], [{ name: rpm, sig: "8:1:1" }], "4.0.0", "4.0.0")).toEqual({
      renames: [],
      built: rpm,
    });
    const old = [{ name: rpm, sig: "8:1:1" }];
    expect(() => pickBuilt(old, old, "4.0.0", "4.0.0")).toThrow(/found 0/);
  });
});

describe("snapshot", () => {
  const scratch = () => mkdtempSync(join(tmpdir(), "bundle-rpm-test-"));

  it("is empty for a folder that doesn't exist yet", () => {
    const dir = scratch();
    try {
      expect(snapshot(join(dir, "rpm"))).toEqual([]);
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });

  it("lists files only, and tells an in-place rewrite of the same bytes apart", () => {
    const dir = scratch();
    try {
      const rpm = join(dir, "PM-3.155.1+alpha-1.x86_64.rpm");
      writeFileSync(rpm, "same bytes");
      // As if written a minute ago, so the rewrite below can't share its mtime.
      const earlier = new Date(Date.now() - 60_000);
      utimesSync(rpm, earlier, earlier);
      mkdirSync(join(dir, "PM-3.155.1+alpha-1.x86_64"));
      const before = snapshot(dir);
      expect(before.map((e) => e.name)).toEqual(["PM-3.155.1+alpha-1.x86_64.rpm"]);

      // Tauri's File::create: the same inode, truncated and written again.
      writeFileSync(rpm, "same bytes");
      const after = snapshot(dir);
      expect(after[0].sig).not.toBe(before[0].sig);
      expect(pickBuilt(before, after, "3.155.1-alpha", "3.155.1+alpha").built).toBe(
        "PM-3.155.1-alpha-1.x86_64.rpm",
      );
    } finally {
      rmSync(dir, { recursive: true, force: true });
    }
  });
});
