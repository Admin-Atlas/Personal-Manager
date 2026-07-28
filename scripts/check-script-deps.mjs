// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Build-tooling dependency gate (INVARIANTS.md I-18). `scripts/` is zero-dependency by rule:
// every gate in here runs in pr.yml's `hygiene` job, which has NO `npm ci` step and therefore no
// node_modules — so a check script that imported an npm package would pass locally and die only
// in CI. That is the hard reason, on top of the soft one (a build script is the easiest place for
// a dependency to arrive unnoticed).
//
// The rule is not "no dependencies ever" — one is already justified (`@huggingface/gguf`, reading
// MoE expert counts out of a binary GGUF header). It is that each one is a DECISION someone made
// on the record: an entry in ALLOWED below, stating the file, the specifier and why. Adding a
// dependency then means editing a file that states the bar, not hoping the next person reads a
// comment in a different file.
//
// Asserted, both directions plus the bar itself:
//   1. every non-`node:`, non-relative import in a tracked `scripts/*.mjs` is in ALLOWED;
//   2. every ALLOWED entry is still imported (a stale exception is a rule nobody is following);
//   3. each allowed specifier is a devDependency (never shipped) at an EXACT pin (no range) —
//      the two properties the existing justification actually claims.
//
// Pure Node built-ins, ESM, no dependencies. (Yes: this gate is bound by the rule it enforces.)

import { readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const read = (rel) => readFileSync(join(repoRoot, rel), "utf8").replace(/\r\n/g, "\n");

// ---------------------------------------------------------------------------------------------
// The allowlist. One entry per (file, specifier). Adding one is the decision; state why.
// ---------------------------------------------------------------------------------------------
const ALLOWED = [
  {
    file: "scripts/generate-local-catalog.mjs",
    specifier: "@huggingface/gguf",
    why: "Reads MoE expert counts out of a binary GGUF header over HTTP range requests. A maintained format library prevents a correctness bug we cannot cheaply verify by hand; dev-only, never shipped, and the generator is deliberately not part of `just check`.",
  },
];

// ---------------------------------------------------------------------------------------------
// Scan. Bias: OVER-match rather than under-match. A specifier written inside a comment is still
// reported, because the dangerous direction is a false PASS — an import the scanner failed to see.
// A false positive fails loudly and is one allowlist line away from resolved.
// ---------------------------------------------------------------------------------------------

/** Every module specifier `src` references: `from "x"`, bare `import "x"`, and `import("x")`. */
function specifiersIn(src) {
  const found = [];
  // `from "x"` — covers `import … from`, `export … from`, and multi-line import braces (the
  // `from` clause lands on its own line in fetch-python.mjs, which a line-anchored regex misses).
  for (const m of src.matchAll(/\bfrom\s*["']([^"']+)["']/g)) found.push(m[1]);
  // Bare side-effect import: `import "x";`
  for (const m of src.matchAll(/(?:^|[\n;])\s*import\s*["']([^"']+)["']/g)) found.push(m[1]);
  // Dynamic: `import("x")` / `await import( "x" )`
  for (const m of src.matchAll(/\bimport\s*\(\s*["']([^"']+)["']\s*\)/g)) found.push(m[1]);
  return found;
}

/** A specifier that needs no permission: a Node built-in, or a path inside the repo. */
const isFree = (spec) =>
  spec.startsWith("node:") || spec.startsWith("./") || spec.startsWith("../");

const files = execFileSync("git", ["ls-files", "scripts/*.mjs"], {
  cwd: repoRoot,
  encoding: "utf8",
})
  .split("\n")
  .map((f) => f.trim())
  .filter(Boolean);

const problems = [];
const seen = new Set(); // `${file}\0${specifier}` actually imported
let totalSpecifiers = 0;

for (const file of files) {
  for (const spec of specifiersIn(read(file))) {
    totalSpecifiers += 1;
    if (isFree(spec)) continue;
    seen.add(`${file}\0${spec}`);
    if (!ALLOWED.some((a) => a.file === file && a.specifier === spec)) {
      problems.push(
        `${file} imports "${spec}" — not a node: builtin, not a repo-relative path, and not in ALLOWED`,
      );
    }
  }
}

// Parser sanity: this repo has ~36 imports across scripts/. If we parsed almost none, the scanner
// broke and every assertion above is vacuously true — which is exactly how a gate passes forever
// while checking nothing.
if (totalSpecifiers < 10) {
  console.error(
    `✗ script-deps: parsed only ${totalSpecifiers} import specifiers across ${files.length} files.\n`,
  );
  console.error("  That is the parser being broken, not the scripts being clean. Fix the scanner.");
  process.exit(1);
}

// Direction 2: no stale exception.
for (const entry of ALLOWED) {
  if (!files.includes(entry.file)) {
    problems.push(
      `ALLOWED lists ${entry.file}, which is not a tracked scripts/*.mjs — drop the entry or fix the path`,
    );
  } else if (!seen.has(`${entry.file}\0${entry.specifier}`)) {
    problems.push(
      `ALLOWED permits "${entry.specifier}" in ${entry.file}, but nothing there imports it — drop the entry`,
    );
  }
}

// Direction 3: the exception must still meet the bar it was granted on.
const pkg = JSON.parse(read("package.json"));
const EXACT = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?$/;
for (const { specifier } of ALLOWED) {
  const dev = pkg.devDependencies?.[specifier];
  const prod = pkg.dependencies?.[specifier];
  if (prod !== undefined) {
    problems.push(
      `"${specifier}" is in dependencies — a scripts/ dependency must be devDependencies only (never shipped)`,
    );
  }
  if (dev === undefined && prod === undefined) {
    problems.push(`"${specifier}" is allowed for scripts/ but is not in package.json at all`);
  } else if (dev !== undefined && !EXACT.test(dev)) {
    problems.push(
      `"${specifier}" is pinned "${dev}" — a scripts/ dependency takes an exact version, no range operator`,
    );
  }
}

if (problems.length) {
  console.error("✗ script-deps: scripts/ dependency rule broken:\n");
  for (const p of problems) console.error(`  • ${p}`);
  console.error(
    "\n  scripts/ is zero-dependency by default (INVARIANTS.md I-18). pr.yml's hygiene job runs\n" +
      "  no `npm ci`, so an unlisted import passes locally and fails only in CI. If the dependency\n" +
      "  is genuinely justified, add it to ALLOWED in this file with the reason — that entry IS the\n" +
      "  decision.",
  );
  process.exit(1);
}

console.log(
  `✓ script-deps: ${totalSpecifiers} imports across ${files.length} scripts, ` +
    `${ALLOWED.length} allowed exception${ALLOWED.length === 1 ? "" : "s"}, all still justified`,
);
