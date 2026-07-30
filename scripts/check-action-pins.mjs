// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Action-pin gate (2026-07-29 audit, batch A1). Both workflow headers state that actions are
// SHA-pinned "(the repo enforces it)". Nothing enforced it. Every `uses:` in the tree happened to
// be pinned correctly, which is the failure mode that matters: the claim was true by habit, and a
// single `uses: foo/bar@v3` would have landed green while the repo went on asserting the opposite.
//
// Why the rule is worth a gate at all: a tag is a MOVING pointer the action's owner can re-point
// after you have vetted it, so `@v4` grants that owner (and anyone who compromises them) write
// access to a runner holding this repo's token — and, in release.yml, one holding `contents: write`
// and the updater signing key. A commit SHA cannot be re-pointed.
//
// Asserted, per `uses:` reference:
//   1. third-party actions are pinned to a 40-hex commit SHA (not a tag, not a branch);
//   2. the pin carries a trailing `#` comment — the human-readable version, which is what makes a
//      wall of hashes reviewable and what Dependabot rewrites when it advances a pin;
//   3. a `docker://` reference carries an image digest rather than a tag;
//   4. local `./…` actions are exempt (they are this repo's own tree, already reviewed).
//
// Plus the floor that keeps the gate honest: if the scanner parses almost nothing, every assertion
// above is vacuously true. That is how a gate passes forever while checking nothing, so a low count
// is a hard failure rather than a clean run.
//
// Pure Node built-ins, ESM, no dependencies (INVARIANTS.md I-18 — pr.yml's hygiene job has no
// node_modules). Importing this module does NOT run it; the entry-point guard is at the bottom.

import { readdirSync, readFileSync } from "node:fs";
import { fileURLToPath, pathToFileURL } from "node:url";
import { dirname, join } from "node:path";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const WORKFLOW_DIR = ".github/workflows";

/** A pin that is a real commit SHA: exactly 40 lowercase hex characters. */
const SHA40 = /^[0-9a-f]{40}$/;

/**
 * Every `uses:` reference in one workflow file, with its line number and trailing comment.
 *
 * Matched as a YAML key at the start of a line (optionally after a `- ` list dash), never by
 * searching for the text `uses:` anywhere — this file's own pr.yml step is literally named
 * "Every workflow `uses:` is SHA-pinned", and a looser scanner would report the step that runs it.
 */
export function findUses(text) {
  const out = [];
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  for (const [i, line] of lines.entries()) {
    const m = line.match(/^\s*(?:-\s+)?uses:\s*(\S+)\s*(#.*)?$/);
    if (!m) continue;
    out.push({ line: i + 1, ref: m[1], comment: (m[2] ?? "").trim() });
  }
  return out;
}

/** Human-readable complaints about one `uses:` reference, or [] when it meets the bar. */
export function problemsForRef({ ref, comment }) {
  // This repo's own composite actions — already in the reviewed tree, nothing to pin to.
  if (ref.startsWith("./")) return [];

  if (ref.startsWith("docker://")) {
    return ref.includes("@sha256:")
      ? []
      : [`docker reference is not digest-pinned — use docker://image@sha256:<digest>`];
  }

  const at = ref.lastIndexOf("@");
  if (at === -1) return [`no version at all — pin it to a 40-character commit SHA`];

  const pin = ref.slice(at + 1);
  const problems = [];
  if (!SHA40.test(pin)) {
    problems.push(
      `pinned to "${pin}", which is a tag or branch, not a commit SHA. A tag can be re-pointed ` +
        `by the action's owner after you vetted it; pin the 40-character SHA instead`,
    );
  }
  if (!comment) {
    problems.push(
      `has no trailing "# vX.Y.Z" comment — without it the pin is an unreviewable hash, and ` +
        `Dependabot has nothing to rewrite when it advances the pin`,
    );
  }
  return problems;
}

/** Scan the workflow directory. Returns `{ problems, refCount, fileCount }`. */
export function scan(root = repoRoot) {
  const dir = join(root, WORKFLOW_DIR);
  const files = readdirSync(dir)
    .filter((f) => f.endsWith(".yml") || f.endsWith(".yaml"))
    .sort();

  const problems = [];
  let refCount = 0;
  for (const file of files) {
    for (const use of findUses(readFileSync(join(dir, file), "utf8"))) {
      refCount += 1;
      for (const p of problemsForRef(use)) {
        problems.push(`${WORKFLOW_DIR}/${file}:${use.line}  ${use.ref}\n      ${p}`);
      }
    }
  }
  return { problems, refCount, fileCount: files.length };
}

function main() {
  const { problems, refCount, fileCount } = scan();

  // The floor. This repo has ~54 `uses:` across 4 workflows; parsing a handful means the scanner
  // broke, not that the workflows got smaller.
  if (refCount < 20 || fileCount < 3) {
    console.error(
      `✗ action-pins: parsed only ${refCount} \`uses:\` references across ${fileCount} workflow files.\n`,
    );
    console.error("  That is the scanner being broken, not the workflows being clean. Fix it.");
    process.exit(1);
  }

  if (problems.length) {
    console.error("✗ action-pins: workflow actions are not SHA-pinned:\n");
    for (const p of problems) console.error(`  • ${p}`);
    console.error(
      "\n  pr.yml and release.yml both state that actions are SHA-pinned and that the repo\n" +
        "  enforces it. This is what enforces it. Pin to the 40-character commit SHA and keep\n" +
        "  the trailing version comment.",
    );
    process.exit(1);
  }

  console.log(
    `✓ action-pins: all ${refCount} \`uses:\` references across ${fileCount} workflows are ` +
      `SHA-pinned with a version comment`,
  );
}

// Run only when invoked directly, so the test can import the pure helpers without scanning.
// pathToFileURL, not import.meta.filename: the latter is not available on every supported Node.
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}
