// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Every gate reports how much it inspected, and the number is never zero.
//
// THE FAILURE THIS EXISTS FOR. A gate that scans an empty set passes. `ls-files` returns nothing
// after a glob typo, a JSON key gets renamed and the loop iterates zero times, a directory moves —
// and the gate prints its tick, exits 0, and is believed. Every one of the checks below already
// PRINTED a count, and not one of them FAILED on a zero, which is the difference between a number
// on the screen and an assertion.
//
// This runs each gate for real against the working tree and holds it to two things: it passes, and
// it says it looked at something. That is deliberately shallow — it is a floor under all of them,
// not a substitute for a gate's own tests (`check-ipc-commands.test.mjs`, `check-model-licences`,
// `check-sidecar-licences`, `check-requirements-lock`, `check-action-pins` each have those).
//
// Excluded on purpose: `check-npm-licenses` reads `node_modules`, so it belongs to the frontend job
// rather than the zero-dependency hygiene set, and the generators/fetchers (`fetch-python`,
// `regen-*`, `generate-*`, `build-updater-manifest`, `verify-updater-*`) reach the network or an
// unbuilt artefact.

import { execFileSync } from "node:child_process";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

const scriptsDir = dirname(fileURLToPath(import.meta.url));
const repoRoot = join(scriptsDir, "..");

/** The offline, zero-dependency gates that run in pr.yml's `hygiene` job. */
const GATES = [
  "check-action-pins",
  "check-ci-membership",
  "check-files-in-place",
  "check-ipc-commands",
  "check-license-subset",
  "check-model-licences",
  "check-node-version",
  "check-requirements-lock",
  "check-script-deps",
  "check-sidecar-licences",
  "check-spdx-headers",
  "check-sync-set",
  "check-version-lockstep",
];

/**
 * The largest integer a gate printed.
 *
 * Version strings would otherwise be read as counts (`3.113.0-alpha` holds a 113), so digits with a
 * `.` on either side are dropped first — the count has to be a standalone number.
 */
function largestCount(output) {
  const withoutVersions = output.replace(/\d+(\.\d+)+(-[a-z0-9.]+)?/gi, " ");
  const numbers = [...withoutVersions.matchAll(/\b(\d+)\b/g)].map((m) => Number(m[1]));
  return numbers.length > 0 ? Math.max(...numbers) : 0;
}

describe("largestCount", () => {
  it("reads a gate's count and ignores the version numbers around it", () => {
    expect(largestCount("✓ lockstep: all 7 files agree on 3.113.0-alpha")).toBe(7);
    expect(largestCount("✓ node-version: all 12 CI pins run Node 24")).toBe(24);
    expect(largestCount("✓ nothing here")).toBe(0);
  });
});

describe("every offline gate passes and says what it inspected", () => {
  for (const gate of GATES) {
    it(`${gate} reports a non-zero count`, () => {
      let stdout;
      try {
        stdout = execFileSync(process.execPath, [join(scriptsDir, `${gate}.mjs`)], {
          cwd: repoRoot,
          encoding: "utf8",
          stdio: ["ignore", "pipe", "pipe"],
        });
      } catch (e) {
        throw new Error(
          `${gate} failed on the working tree:\n${e.stdout ?? ""}\n${e.stderr ?? ""}`,
          {
            cause: e,
          },
        );
      }
      expect(
        largestCount(stdout),
        `${gate} passed without naming how much it inspected — a gate that scans an empty set ` +
          `reports the same green as one that scanned everything:\n${stdout}`,
      ).toBeGreaterThan(0);
    });
  }

  it("covers every check-* script in scripts/, so a new gate cannot skip this floor", () => {
    // The list above is hand-maintained, which is exactly the kind of list that rots. This is the
    // guard on the guard: a new `check-*.mjs` has to be added here or explicitly excused.
    const EXCLUDED = new Set([
      // Reads node_modules — runs in the frontend job, which has done `npm ci`.
      "check-npm-licenses",
    ]);
    const present = execFileSync("git", ["ls-files", "scripts/check-*.mjs"], {
      cwd: repoRoot,
      encoding: "utf8",
    })
      .split("\n")
      .filter(Boolean)
      .map((p) => p.replace(/^scripts\//, "").replace(/\.mjs$/, ""))
      .filter((n) => !n.endsWith(".test"));

    expect(present.length).toBeGreaterThan(10);
    const missing = present.filter((n) => !GATES.includes(n) && !EXCLUDED.has(n));
    expect(missing, "new gate(s) not covered by the non-zero floor").toEqual([]);
  });
});
