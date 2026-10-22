// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The requirements-lock gate's own rules. The gate exists because a lock that has drifted from its
// inputs is INVISIBLY broken — it stays perfectly valid, pip still installs it happily under
// `--require-hashes`, it is just no longer the resolution anyone reviewed. So the drift detection is
// the part most worth pinning, alongside the two properties that make `--require-hashes` mean
// anything at all: every entry pinned with `==`, and every entry carrying a hash.
//
// Two details here bit during development and are locked as regressions:
//   * uv normalises names (`openTSNE` -> `opentsne`) while the Rust pin keeps the original case, so
//     every comparison has to be PEP 503-normalised;
//   * a name can appear MORE THAN ONCE under different markers (numpy resolves two ways across
//     the supported Python range), so "find the entry" is never a single lookup.
//
// Importing the module does not run the gate — entry-point guard at the bottom of it.

import { readFileSync } from "node:fs";
import { join } from "node:path";

import { describe, expect, it } from "vitest";

import {
  normalise,
  overrideProblems,
  parseEntries,
  parseHeader,
  parseRequirements,
  problemsForLock,
  readRustFacts,
  scan,
  sha256,
} from "./check-requirements-lock.mjs";

const H = "a".repeat(64);
const H2 = "b".repeat(64);

/** A minimal but structurally real lock: header stamps, a forked pin, markers, hashes. */
function lock({ floor = "3.11", inputSha, extra = "", body } = {}) {
  return [
    "# PM sidecar dependency lock - GENERATED, DO NOT EDIT BY HAND.",
    "#",
    "# pm-lock: 1",
    `# pm-python-floor: ${floor}`,
    `# pm-input: sidecar/requirements.txt@sha256:${inputSha}`,
    extra,
    body ??
      [
        `defusedxml==0.7.1 \\`,
        `    --hash=sha256:${H}`,
        `    # via -r sidecar/requirements.txt`,
        `numpy==2.2.6 ; python_full_version < '3.12' \\`,
        `    --hash=sha256:${H2}`,
        `numpy==2.5.1 ; python_full_version >= '3.12' \\`,
        `    --hash=sha256:${H}`,
      ].join("\n"),
  ]
    .filter(Boolean)
    .join("\n");
}

const REQS = "# a comment\nmarkitdown[pdf,docx]==0.1.6\ndefusedxml==0.7.1\n";

describe("normalise", () => {
  it("folds case and separator runs the way PEP 503 does", () => {
    // The live case: Rust pins `openTSNE==1.0.4`, uv writes `opentsne==1.0.4`.
    expect(normalise("openTSNE")).toBe("opentsne");
    expect(normalise("pillow_heif")).toBe("pillow-heif");
    expect(normalise("zope..interface")).toBe("zope-interface");
  });
});

describe("sha256", () => {
  it("hashes CRLF and LF identically", () => {
    // Regression: `.gitattributes` pins the repo to eol=lf, but a Windows working copy can hold
    // CRLF. Hashing raw bytes stamped a digest only the generating machine could reproduce — green
    // locally, red on the Linux runner within seconds.
    expect(sha256("markitdown==0.1.6\r\ndefusedxml==0.7.1\r\n")).toBe(
      sha256("markitdown==0.1.6\ndefusedxml==0.7.1\n"),
    );
  });
});

describe("parseEntries", () => {
  it("reads a pin, its marker and its hashes", () => {
    const entries = parseEntries(
      `foo==1.2.3 ; sys_platform == 'win32' \\\n    --hash=sha256:${H}\n`,
    );
    expect(entries).toEqual([
      {
        line: 1,
        name: "foo",
        operator: "==",
        version: "1.2.3",
        marker: "sys_platform == 'win32'",
        hashes: [H],
      },
    ]);
  });

  it("keeps every fork of a name that resolves differently per interpreter", () => {
    const entries = parseEntries(parseEntriesFixture());
    expect(entries.filter((e) => e.name === "numpy").map((e) => e.version)).toEqual([
      "2.2.6",
      "2.5.1",
    ]);
  });

  it("collects multiple hashes for one entry", () => {
    const entries = parseEntries(
      `foo==1.0 \\\n    --hash=sha256:${H} \\\n    --hash=sha256:${H2}\n`,
    );
    expect(entries[0].hashes).toEqual([H, H2]);
  });

  it("reads CRLF the same as LF", () => {
    // .gitattributes normalises the tree to LF, but uv writes CRLF on Windows and a stray \r would
    // land inside the captured version string.
    const entries = parseEntries(`foo==1.2.3 \\\r\n    --hash=sha256:${H}\r\n`);
    expect(entries[0].version).toBe("1.2.3");
    expect(entries[0].hashes).toEqual([H]);
  });

  it("does not attach a later entry's hashes to an earlier one", () => {
    const entries = parseEntries(
      `foo==1.0 \\\n    --hash=sha256:${H}\nbar==2.0 \\\n    --hash=sha256:${H2}\n`,
    );
    expect(entries.map((e) => e.hashes)).toEqual([[H], [H2]]);
  });
});

function parseEntriesFixture() {
  return lock({ inputSha: sha256(REQS) });
}

describe("parseRequirements", () => {
  it("strips extras and skips comments and blank lines", () => {
    expect(parseRequirements(REQS)).toEqual([
      { name: "markitdown", version: "0.1.6", raw: "markitdown[pdf,docx]==0.1.6" },
      { name: "defusedxml", version: "0.7.1", raw: "defusedxml==0.7.1" },
    ]);
  });
});

describe("readRustFacts", () => {
  it("reads the floor and both optional pin constants", () => {
    const rust = [
      "const MIN_PYTHON: (u32, u32) = (3, 11);",
      'const OPTIONAL_TSNE_PIN: &str = "openTSNE==1.0.4";',
      'const OPTIONAL_OCR_PINS: &[&str] = &["rapidocr==3.9.2", "pi-heif==1.4.0"];',
    ].join("\n");
    expect(readRustFacts(rust)).toEqual({
      floor: "3.11",
      pins: { tsne: ["openTSNE==1.0.4"], ocr: ["rapidocr==3.9.2", "pi-heif==1.4.0"] },
    });
  });

  it("throws rather than guessing when the constants move", () => {
    expect(() => readRustFacts("fn main() {}")).toThrow(/MIN_PYTHON/);
  });
});

describe("parseHeader", () => {
  it("reads the stamps, including the optional-lock ones", () => {
    const text = lock({
      inputSha: H,
      extra: `# pm-constraint: sidecar/requirements.lock@sha256:${H2}\n# pm-pins: rapidocr==3.9.2 pi-heif==1.4.0`,
    });
    const header = parseHeader(text);
    expect(header.problems).toEqual([]);
    expect(header.floor).toBe("3.11");
    expect(header.input).toEqual({ path: "sidecar/requirements.txt", sha: H });
    expect(header.constraint).toEqual({ path: "sidecar/requirements.lock", sha: H2 });
    expect(header.override).toBeNull();
    expect(header.pins).toEqual(["rapidocr==3.9.2", "pi-heif==1.4.0"]);
  });

  it("reads the base lock's override stamp", () => {
    const header = parseHeader(
      lock({ inputSha: H, extra: `# pm-override: sidecar/requirements-override.txt@sha256:${H2}` }),
    );
    expect(header.problems).toEqual([]);
    expect(header.override).toEqual({ path: "sidecar/requirements-override.txt", sha: H2 });
  });

  it("rejects a malformed override stamp rather than reading it as absent", () => {
    const header = parseHeader(
      lock({ inputSha: H, extra: "# pm-override: sidecar/requirements-override.txt" }),
    );
    expect(header.problems.join(" ")).toMatch(/malformed `# pm-override:`/);
  });

  it("rejects a hand-written file with no stamps at all", () => {
    const header = parseHeader("foo==1.0\n");
    expect(header.problems.join(" ")).toMatch(/pm-lock/);
    expect(header.problems.join(" ")).toMatch(/pm-input/);
  });
});

describe("problemsForLock", () => {
  const inputs = { "sidecar/requirements.txt": REQS };
  const base = () => ({
    path: "sidecar/fixture.lock", // not one of the three real locks, so no entry floor applies
    text: lock({ inputSha: sha256(REQS) }),
    floor: "3.11",
    inputs,
    expectedPins: parseRequirements(REQS).filter((p) => p.name === "defusedxml"),
  });

  it("passes a lock that is current", () => {
    expect(problemsForLock(base())).toEqual([]);
  });

  it("catches an input edited without regenerating — the whole point of the gate", () => {
    const problems = problemsForLock({
      ...base(),
      inputs: { "sidecar/requirements.txt": REQS + "pillow==12.3.0\n" },
    });
    expect(problems.join(" ")).toMatch(/has changed since this lock was generated/);
  });

  it("catches an override file edited without regenerating", () => {
    const OVERRIDE = "sidecar/requirements-override.txt";
    const text = lock({
      inputSha: sha256(REQS),
      extra: `# pm-override: ${OVERRIDE}@sha256:${sha256("pillow==13.0.0\n")}`,
    });
    const current = { ...inputs, [OVERRIDE]: "pillow==13.0.0\n" };
    expect(problemsForLock({ ...base(), text, inputs: current })).toEqual([]);
    const edited = { ...inputs, [OVERRIDE]: "pillow==13.0.1\n" };
    expect(problemsForLock({ ...base(), text, inputs: edited }).join(" ")).toMatch(
      /requirements-override\.txt has changed since this lock was generated/,
    );
  });

  it("catches a MIN_PYTHON floor that moved without a regeneration", () => {
    expect(problemsForLock({ ...base(), floor: "3.12" }).join(" ")).toMatch(/MIN_PYTHON/);
  });

  it("catches an entry with no hash, which --require-hashes cannot verify", () => {
    const text = lock({ inputSha: sha256(REQS), body: "defusedxml==0.7.1\n" });
    expect(problemsForLock({ ...base(), text }).join(" ")).toMatch(/carries no --hash/);
  });

  it("catches a range where a pin belongs", () => {
    const text = lock({
      inputSha: sha256(REQS),
      body: `defusedxml>=0.7.1 \\\n    --hash=sha256:${H}\n`,
    });
    expect(problemsForLock({ ...base(), text }).join(" ")).toMatch(/is not pinned/);
  });

  it("catches a required pin missing from the lock entirely", () => {
    const text = lock({ inputSha: sha256(REQS), body: `other==1.0 \\\n    --hash=sha256:${H}\n` });
    expect(problemsForLock({ ...base(), text }).join(" ")).toMatch(/does not appear in this lock/);
  });

  it("catches a lock resolving a top-level pin to a different version", () => {
    const text = lock({
      inputSha: sha256(REQS),
      body: `defusedxml==0.7.0 \\\n    --hash=sha256:${H}\n`,
    });
    expect(problemsForLock({ ...base(), text }).join(" ")).toMatch(
      /pinned to 0\.7\.1 but the lock resolves it to 0\.7\.0/,
    );
  });

  it("fails a truncated base lock instead of reporting a clean scan of nothing", () => {
    // The vacuous-pass guard: if the entry parser ever stops matching, the floor turns a silent
    // "0 problems" into a loud failure.
    const problems = problemsForLock({ ...base(), path: "sidecar/requirements.lock" });
    expect(problems.join(" ")).toMatch(/is truncated, or this parser has stopped matching/);
  });
});

describe("overrideProblems", () => {
  // The override exists to resolve PAST a dependency's declared bound (fastembed 0.9.0's
  // `pillow<13`). Two ways that goes wrong quietly: a base lock regenerated without it (pillow drops
  // back below 13, or the resolution simply fails somewhere nobody looks), and an override that grows
  // into a blanket hole — a range, or a package PM never pins and audits by name.
  const OVERRIDE = "sidecar/requirements-override.txt";
  const REQ = "# top-level\nfastembed==0.9.0\npillow==13.0.0\n";
  const stamped = parseHeader(
    lock({ inputSha: H, extra: `# pm-override: ${OVERRIDE}@sha256:${H2}` }),
  );
  const ok = "# why, and when to remove it\npillow==13.0.0\n";

  it("passes a stamped base lock whose override pins what requirements.txt pins", () => {
    expect(overrideProblems({ header: stamped, overrideText: ok, requirementsText: REQ })).toEqual(
      [],
    );
  });

  it("passes an override file with no pins left in it", () => {
    // The removal condition empties the file and keeps it, so the stamp still has something to hash.
    expect(
      overrideProblems({ header: stamped, overrideText: "# none\n", requirementsText: REQ }),
    ).toEqual([]);
  });

  it("fails a base lock resolved without the override", () => {
    const unstamped = parseHeader(lock({ inputSha: H }));
    expect(
      overrideProblems({ header: unstamped, overrideText: ok, requirementsText: REQ }).join(" "),
    ).toMatch(/no `# pm-override:` stamp/);
  });

  it("fails a stamp that names some other file", () => {
    const other = parseHeader(
      lock({ inputSha: H, extra: `# pm-override: sidecar/requirements.txt@sha256:${H2}` }),
    );
    expect(
      overrideProblems({ header: other, overrideText: ok, requirementsText: REQ }).join(" "),
    ).toMatch(/the base lock's override is sidecar\/requirements-override\.txt/);
  });

  it("fails an override that is a range rather than a pin", () => {
    expect(
      overrideProblems({
        header: stamped,
        overrideText: "pillow>=13\n",
        requirementsText: REQ,
      }).join(" "),
    ).toMatch(/is not an exact `==` pin/);
  });

  it("fails an override on a package requirements.txt does not pin", () => {
    expect(
      overrideProblems({
        header: stamped,
        overrideText: "numpy==2.3.0\n",
        requirementsText: REQ,
      }).join(" "),
    ).toMatch(/`numpy==2\.3\.0` is not pinned at 2\.3\.0 in sidecar\/requirements\.txt/);
  });

  it("fails an override at a different version from the top-level pin", () => {
    expect(
      overrideProblems({
        header: stamped,
        overrideText: "Pillow==13.0.1\n",
        requirementsText: REQ,
      }).join(" "),
    ).toMatch(/is not pinned at 13\.0\.1/);
  });
});

describe("scan", () => {
  const root = new URL("..", import.meta.url).pathname.replace(/^\/([A-Za-z]:)/, "$1");

  it("passes on the real tree and parses a plausible number of entries", () => {
    const { failures, entryCount } = scan(root);
    expect(failures).toEqual([]);
    // Sixty-odd base packages plus both optional components. A collapse to a handful means the
    // parser broke; this is the same floor `problemsForLock` enforces, asserted end to end.
    expect(entryCount).toBeGreaterThan(80);
  });

  it("runs the override check on the real base lock", () => {
    // The real tree with only the base lock's override stamp removed: what a regeneration that
    // forgot `--override` would leave behind.
    const real = (rel) => readFileSync(join(root, rel), "utf8");
    const read = (rel) =>
      rel === "sidecar/requirements.lock"
        ? real(rel).replace(/^# pm-override:.*\n/m, "")
        : real(rel);
    expect(scan(root, read).failures.join("\n")).toMatch(
      /sidecar\/requirements\.lock: no `# pm-override:` stamp/,
    );
  });
});
