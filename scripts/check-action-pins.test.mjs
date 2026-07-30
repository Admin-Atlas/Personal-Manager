// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The action-pin gate's own rules. A gate that only ever runs against a clean tree cannot tell you
// whether it would catch anything — the SHA-pin claim in pr.yml's header sat there for months being
// true by habit with nothing behind it, which is the same failure one level up.
//
// Two things are worth pinning here beyond "a tag fails". First, the SCANNER: it must find a `uses:`
// key and must not find the words `uses:` in prose, because pr.yml now contains a step named
// "Every workflow `uses:` is SHA-pinned" and a text-searching version of this gate would report the
// step that runs it. Second, the FLOOR: `scan()` reports how much it parsed so a broken scanner
// fails loudly instead of passing vacuously.
//
// Importing the module does not run the gate (entry-point guard at the bottom of it), so nothing
// here touches .github/workflows.

import { describe, expect, it } from "vitest";

import { findUses, problemsForRef, scan } from "./check-action-pins.mjs";

const pinned = "actions/checkout@3d3c42e5aac5ba805825da76410c181273ba90b1";
const ok = { ref: pinned, comment: "# v7.0.1" };

describe("findUses", () => {
  it("finds a `uses:` key as a list item and as a plain key", () => {
    const found = findUses(`jobs:\n  a:\n    steps:\n      - uses: ${pinned} # v7.0.1\n`);
    expect(found).toEqual([{ line: 4, ref: pinned, comment: "# v7.0.1" }]);
  });

  it("does not treat the words `uses:` in a step name as a reference", () => {
    // The live case: pr.yml's own step for this gate is named with `uses:` inside backticks.
    const yaml = `      - name: Every workflow \`uses:\` is SHA-pinned\n        run: just action-pins\n`;
    expect(findUses(yaml)).toEqual([]);
  });

  it("reads CRLF files the same as LF ones", () => {
    // .gitattributes normalises to LF in the repo, but a Windows checkout can still hand us CRLF —
    // and a stray \r would land inside the captured comment and silently change every assertion.
    const found = findUses(`      - uses: ${pinned} # v7.0.1\r\n`);
    expect(found).toEqual([{ line: 1, ref: pinned, comment: "# v7.0.1" }]);
  });
});

describe("problemsForRef", () => {
  it("accepts a 40-hex SHA carrying a version comment", () => {
    expect(problemsForRef(ok)).toEqual([]);
  });

  it("accepts a non-version comment, because one real pin has one", () => {
    // dtolnay/rust-toolchain is pinned with "# stable (re-pin to advance Rust)". The bar is that a
    // human wrote something reviewable next to the hash, not that it matches a version grammar.
    expect(problemsForRef({ ref: pinned, comment: "# stable (re-pin to advance Rust)" })).toEqual(
      [],
    );
  });

  it("rejects a tag pin", () => {
    const problems = problemsForRef({ ref: "actions/checkout@v4", comment: "# v4" });
    expect(problems).toHaveLength(1);
    expect(problems[0]).toMatch(/not a commit SHA/);
  });

  it("rejects a branch pin", () => {
    expect(problemsForRef({ ref: "foo/bar@main", comment: "# main" })).toHaveLength(1);
  });

  it("rejects a SHA with no version comment", () => {
    const problems = problemsForRef({ ref: pinned, comment: "" });
    expect(problems).toHaveLength(1);
    expect(problems[0]).toMatch(/no trailing/);
  });

  it("rejects a short SHA — an abbreviation is still ambiguous", () => {
    expect(problemsForRef({ ref: "actions/checkout@3d3c42e", comment: "# v7.0.1" })).toHaveLength(
      1,
    );
  });

  it("rejects an uppercase SHA, which git will not resolve as written", () => {
    expect(problemsForRef({ ref: pinned.toUpperCase(), comment: "# v7.0.1" })).toHaveLength(1);
  });

  it("rejects a reference with no version at all", () => {
    expect(problemsForRef({ ref: "actions/checkout", comment: "" })).toHaveLength(1);
  });

  it("exempts this repo's own composite actions", () => {
    expect(problemsForRef({ ref: "./.github/actions/setup", comment: "" })).toEqual([]);
  });

  it("requires a digest on a docker reference, not a tag", () => {
    expect(problemsForRef({ ref: "docker://alpine:3.20", comment: "" })).toHaveLength(1);
    expect(
      problemsForRef({ ref: "docker://alpine@sha256:" + "a".repeat(64), comment: "" }),
    ).toEqual([]);
  });

  it("reports both faults at once when a pin is a tag AND uncommented", () => {
    expect(problemsForRef({ ref: "actions/checkout@v4", comment: "" })).toHaveLength(2);
  });
});

describe("scan", () => {
  it("reports what it parsed, so an empty run cannot look like a clean one", () => {
    const { problems, refCount, fileCount } = scan();
    expect(problems).toEqual([]);
    // The same floor main() enforces. If the workflows genuinely shrink below this, that is a
    // deliberate change and this number moves with it.
    expect(refCount).toBeGreaterThanOrEqual(20);
    expect(fileCount).toBeGreaterThanOrEqual(3);
  });
});
