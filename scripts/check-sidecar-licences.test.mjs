// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The sidecar-licences gate's own rules. Two things here are easy to get wrong and were:
//
//   * a `--universal` lock pins one package SEVERAL times, once per fork of the resolution (numpy
//     three ways across the supported Python range). The first draft treated a second version as a
//     conflict and reported thirteen failures on a perfectly correct tree. Every one of those
//     versions installs on somebody's machine, so every one needs a reviewed licence — hence
//     `versions` throughout, and the tests below that pin it.
//   * the real cross-lock invariant is narrower: the optional locks are compiled `--constraint`ed
//     to the base lock, so they may pin FEWER versions but never a version the base lock lacks.
//
// Importing the module does not run the gate — entry-point guard at the bottom of it.

import { mkdtempSync, mkdirSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import { lockedPackages, scan, LOCKS, LICENCES_FILE } from "./check-sidecar-licences.mjs";

/** A lock body with the pins given as `[name, version, marker?]`. */
function lock(pins) {
  return pins
    .map(([name, version, marker]) => {
      const suffix = marker ? ` ; ${marker}` : "";
      return `${name}==${version}${suffix} \\\n    --hash=sha256:${"a".repeat(64)}`;
    })
    .join("\n");
}

/** A throwaway tree holding the three locks and a licences file. */
function fixture({ base, ocr = [], tsne = [], packages }) {
  const root = mkdtempSync(join(tmpdir(), "pm-licences-"));
  mkdirSync(join(root, "sidecar"));
  writeFileSync(join(root, LOCKS[0]), lock(base));
  writeFileSync(join(root, LOCKS[1]), lock(ocr));
  writeFileSync(join(root, LOCKS[2]), lock(tsne));
  writeFileSync(join(root, LICENCES_FILE), JSON.stringify({ version: 1, packages }));
  return root;
}

/** Enough packages to clear the floor, all trivially fine, so a test's own case is what fails. */
function filler(count) {
  const pins = [];
  const packages = {};
  for (let i = 0; i < count; i++) {
    pins.push([`filler${i}`, "1.0.0"]);
    packages[`filler${i}`] = { versions: ["1.0.0"], licence: "MIT" };
  }
  return { pins, packages };
}

describe("lockedPackages", () => {
  it("keeps every version a package is pinned at, not just the first", () => {
    const root = fixture({
      base: [
        ["numpy", "2.2.6", "python_full_version < '3.11'"],
        ["numpy", "2.4.6", "python_full_version >= '3.11'"],
      ],
      packages: {},
    });
    const numpy = lockedPackages(root).find((p) => p.name === "numpy");
    expect(numpy.versions).toEqual(["2.2.6", "2.4.6"]);
    expect(lockedPackages(root).conflicts).toEqual([]);
  });

  it("merges the same package across locks without duplicating it", () => {
    const root = fixture({
      base: [["numpy", "2.4.6"]],
      ocr: [["numpy", "2.4.6"]],
      tsne: [["numpy", "2.4.6"]],
      packages: {},
    });
    const packages = lockedPackages(root);
    expect(packages.filter((p) => p.name === "numpy")).toHaveLength(1);
    expect(packages[0].versions).toEqual(["2.4.6"]);
  });

  it("normalises names the way PEP 503 does, so pi_heif and pi-heif are one package", () => {
    const root = fixture({
      base: [["pi_heif", "1.4.0"]],
      ocr: [["pi-heif", "1.4.0"]],
      packages: {},
    });
    expect(lockedPackages(root).map((p) => p.name)).toEqual(["pi-heif"]);
  });

  it("lets an optional lock pin a SUBSET of the base lock's versions", () => {
    // Narrower markers are normal: the OCR lock need not repeat a fork the base lock resolved.
    const root = fixture({
      base: [
        ["numpy", "2.2.6"],
        ["numpy", "2.4.6"],
      ],
      ocr: [["numpy", "2.4.6"]],
      packages: {},
    });
    expect(lockedPackages(root).conflicts).toEqual([]);
  });

  it("catches an optional lock moving a package the base venv already runs", () => {
    // The whole point of compiling the optional locks `--constraint sidecar/requirements.lock`.
    const root = fixture({
      base: [["numpy", "2.4.6"]],
      ocr: [["numpy", "2.5.1"]],
      packages: {},
    });
    expect(lockedPackages(root).conflicts.join(" ")).toMatch(
      /pins numpy 2\.5\.1, which sidecar\/requirements\.lock does not pin/,
    );
  });

  it("ignores a package only an optional component has", () => {
    const root = fixture({ base: [], ocr: [["rapidocr", "3.9.2"]], packages: {} });
    expect(lockedPackages(root).conflicts).toEqual([]);
  });
});

describe("scan", () => {
  it("passes on the real tree, with every package reviewed", () => {
    const root = new URL("..", import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1");
    const { problems, count } = scan(root);
    expect(problems).toEqual([]);
    // Around eighty across the three locks. A collapse to a handful means the parser broke.
    expect(count).toBeGreaterThan(60);
  });

  it("fails a package that is locked but absent from the licence file", () => {
    const { pins, packages } = filler(70);
    const root = fixture({ base: [...pins, ["newthing", "1.0.0"]], packages });
    expect(scan(root).problems.join(" ")).toMatch(
      /newthing \(1\.0\.0\) is in the locks but not in/,
    );
  });

  it("fails a version that was never reviewed, even when the package was", () => {
    // The case a per-package check would wave through: numpy's licence was read at 2.4.6, then a
    // regenerated lock added a 2.5.1 fork whose terms nobody looked at.
    const { pins, packages } = filler(70);
    const root = fixture({
      base: [...pins, ["numpy", "2.4.6"], ["numpy", "2.5.1"]],
      packages: { ...packages, numpy: { versions: ["2.4.6"], licence: "BSD-3-Clause" } },
    });
    expect(scan(root).problems.join(" ")).toMatch(/2\.5\.1 carries no reviewed licence/);
  });

  it("fails a package whose licence is still blank", () => {
    const { pins, packages } = filler(70);
    const root = fixture({
      base: [...pins, ["mystery", "1.0.0"]],
      packages: { ...packages, mystery: { versions: ["1.0.0"], licence: null } },
    });
    expect(scan(root).problems.join(" ")).toMatch(/mystery \(1\.0\.0\) has no reviewed licence/);
  });

  it("fails a licence outside the accepted set", () => {
    // Exactly what the pillow-heif -> pi-heif swap was about: GPL is not on the list, and adding a
    // package under one has to be a conversation rather than a quiet edit.
    const { pins, packages } = filler(70);
    const root = fixture({
      base: [...pins, ["copyleft", "1.0.0"]],
      packages: { ...packages, copyleft: { versions: ["1.0.0"], licence: "GPL-2.0-only" } },
    });
    expect(scan(root).problems.join(" ")).toMatch(
      /copyleft \(1\.0\.0\) is GPL-2\.0-only, which is not in this file's accepted set/,
    );
  });

  it("accepts a compound expression only when every part is accepted", () => {
    const { pins, packages } = filler(70);
    const ok = fixture({
      base: [...pins, ["numpyish", "1.0.0"]],
      packages: {
        ...packages,
        numpyish: { versions: ["1.0.0"], licence: "BSD-3-Clause AND 0BSD AND MIT AND Zlib" },
      },
    });
    expect(scan(ok).problems).toEqual([]);

    const bad = fixture({
      base: [...pins, ["numpyish", "1.0.0"]],
      packages: {
        ...packages,
        numpyish: { versions: ["1.0.0"], licence: "BSD-3-Clause AND GPL-3.0-only" },
      },
    });
    expect(scan(bad).problems.join(" ")).toMatch(/is not in this file's accepted set/);
  });

  it("fails an entry left behind after its package leaves the locks", () => {
    const { pins, packages } = filler(70);
    const root = fixture({
      base: pins,
      packages: { ...packages, "pillow-heif": { versions: ["1.5.0"], licence: "BSD-3-Clause" } },
    });
    expect(scan(root).problems.join(" ")).toMatch(
      /still lists pillow-heif, which no longer appears in any lock/,
    );
  });

  it("fails a truncated set of locks rather than reporting a clean scan of nothing", () => {
    const root = fixture({ base: [["only", "1.0.0"]], packages: {} });
    expect(scan(root).problems.join(" ")).toMatch(/a lock is truncated, or the parser has stopped/);
  });
});
