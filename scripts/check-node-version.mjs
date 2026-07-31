// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Every CI job runs the Node major that package.json declares.
//
// WHY. The Node version lived in eleven hand-maintained `node-version:` pins across five workflow
// files and nowhere else — no `engines`, no `.nvmrc`. Nothing connected them to each other or to
// the version a developer actually runs, so the two drifted apart silently: this repo was being
// developed on Node 24 while every CI job built, tested and BUNDLED on Node 20, months after Node
// 20 went end-of-life. A green PR therefore said nothing about the runtime the release was cut on.
//
// `engines.node` is now the single declaration, and this makes it binding. Bumping Node means
// editing package.json and being told, precisely, which pins still disagree.
//
// No separate test file, deliberately: this compares two integers read out of two files, and the
// only interesting failure mode — a scanner that quietly matches nothing and reports a clean sweep
// — is covered by the parse floor below rather than by a test. The gates with real parsing
// (`check-action-pins`, `check-requirements-lock`) carry test files because they have grammar to
// get wrong.

import { readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const WORKFLOWS = ".github/workflows";

/** Fewest pins that can legitimately exist. Below this, assume the scan broke, not that CI shrank. */
const PIN_FLOOR = 8;

/** The declared major from an `engines.node` range. Only the shapes this repo actually uses. */
export function declaredMajor(engines) {
  const m = /^>=\s*(\d+)/.exec(engines ?? "");
  if (!m) {
    throw new Error(
      `package.json engines.node is ${JSON.stringify(engines)}; this check understands ">=<major>" ` +
        `(e.g. ">=24"). Widen it here deliberately rather than loosening the declaration.`,
    );
  }
  return Number(m[1]);
}

/** Every `node-version:` pin in a workflow file, with its line number. */
export function findPins(text) {
  const out = [];
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  for (const [i, line] of lines.entries()) {
    const m = line.match(/^\s*node-version:\s*["']?([\d.x]+)["']?\s*$/);
    if (m) out.push({ line: i + 1, value: m[1] });
  }
  return out;
}

export function scan(root) {
  const pkg = JSON.parse(readFileSync(join(root, "package.json"), "utf8"));
  const major = declaredMajor(pkg.engines?.node);
  const problems = [];
  let pinCount = 0;

  for (const file of readdirSync(join(root, WORKFLOWS)).filter((f) => /\.ya?ml$/.test(f))) {
    const rel = `${WORKFLOWS}/${file}`;
    for (const pin of findPins(readFileSync(join(root, rel), "utf8"))) {
      pinCount += 1;
      // Majors only. `24` and `24.x` both mean "latest 24"; a full `24.1.2` would pin CI to one
      // patch and quietly stop picking up security releases, so it is refused as well.
      if (pin.value !== String(major) && pin.value !== `${major}.x`) {
        problems.push(
          `${rel}:${pin.line}: node-version is ${pin.value}, but package.json declares node ${major} ` +
            `— every CI job must run the declared major`,
        );
      }
    }
  }

  if (pinCount < PIN_FLOOR) {
    problems.push(
      `only ${pinCount} \`node-version:\` pins found (expected at least ${PIN_FLOOR}) — the scan is ` +
        `broken, or a workflow stopped setting up Node`,
    );
  }
  return { problems, pinCount, major };
}

function main() {
  const { problems, pinCount, major } = scan(repoRoot);
  if (problems.length > 0) {
    console.error("✗ node-version:\n");
    for (const p of problems) console.error(`  ${p}`);
    process.exit(1);
  }
  console.log(
    `✓ node-version: all ${pinCount} CI pins run Node ${major}, as package.json declares`,
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}
