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
//
// `pinExempt` opts an entry out of the exact-version rule and REQUIRES a reason, so a loosened pin
// is a visible decision rather than a silent one.
// ---------------------------------------------------------------------------------------------
const ALLOWED = [
  {
    file: "scripts/generate-local-catalog.mjs",
    specifier: "@huggingface/gguf",
    why: "Reads MoE expert counts out of a binary GGUF header over HTTP range requests. A maintained format library prevents a correctness bug we cannot cheaply verify by hand; dev-only, never shipped, and the generator is deliberately not part of `just check`.",
  },
  {
    file: "scripts/check-action-pins.test.mjs",
    specifier: "vitest",
    why: "The repo's existing test runner, reached exactly as the catalog generator's test below reaches it. Adds no new dependency — `just frontend-test` already collects this file through a vitest include glob.",
    pinExempt:
      "Same reason as the entry below: vitest is the repo-wide test runner on a `^` range, governed by the normal npm/Dependabot flow. A scripts/ test must not be the thing that dictates the whole repo's runner version.",
  },
  {
    file: "scripts/check-requirements-lock.test.mjs",
    specifier: "vitest",
    why: "The repo's existing test runner, on the same terms as the two entries around it. The gate it tests is the one standing between a user's machine and an unverified Python dependency, so it is worth testing properly; `just frontend-test` already collects this file through a vitest include glob, so no dependency is added.",
    pinExempt:
      "Same reason as the neighbouring entries: vitest is the repo-wide test runner on a `^` range, governed by the normal npm/Dependabot flow. A scripts/ test must not dictate the whole repo's runner version.",
  },
  {
    file: "scripts/check-sidecar-licences.test.mjs",
    specifier: "vitest",
    why: "The repo's existing test runner, on the same terms as the entries around it. This gate decides whether a Python package's licence has been read by a human before PM tells a user's machine to install it, and its version model (one package, several pinned versions behind environment markers) is subtle enough to have been wrong once already; `just frontend-test` already collects this file through a vitest include glob, so no dependency is added.",
    pinExempt:
      "Same reason as the neighbouring entries: vitest is the repo-wide test runner on a `^` range, governed by the normal npm/Dependabot flow. A scripts/ test must not dictate the whole repo's runner version.",
  },
  {
    file: "scripts/check-model-licences.test.mjs",
    specifier: "vitest",
    why: "The repo's existing test runner, on the same terms as the entries around it. This gate stands between a user and a model download whose publisher terms nobody read, and the property it turns on — that the catalogue's COPY of a licence still matches the ledger a human approved — is invisible without a test that breaks it; `just frontend-test` already collects this file through a vitest include glob, so no dependency is added.",
    pinExempt:
      "Same reason as the neighbouring entries: vitest is the repo-wide test runner on a `^` range, governed by the normal npm/Dependabot flow. A scripts/ test must not dictate the whole repo's runner version.",
  },
  {
    file: "scripts/check-ipc-commands.test.mjs",
    specifier: "vitest",
    why: "The repo's existing test runner, on the same terms as the entries around it. This gate's whole substance is its extraction, and the extraction was wrong twice before it ran clean — each time by silently skipping a real call site and then reporting its command as missing from the backend. A gate that can stop seeing part of its subject has to be tested against the shapes that broke it; `just frontend-test` already collects this file through a vitest include glob, so no dependency is added.",
    pinExempt:
      "Same reason as the neighbouring entries: vitest is the repo-wide test runner on a `^` range, governed by the normal npm/Dependabot flow. A scripts/ test must not dictate the whole repo's runner version.",
  },
  {
    file: "scripts/gates-inspect-something.test.mjs",
    specifier: "vitest",
    why: "The repo's existing test runner, on the same terms as the entries around it. This is the floor under every other gate — each one already PRINTED how much it inspected, and not one FAILED on a zero, so a glob typo or a moved directory would have left a gate scanning nothing and still reporting green; `just frontend-test` already collects this file through a vitest include glob, so no dependency is added.",
    pinExempt:
      "Same reason as the neighbouring entries: vitest is the repo-wide test runner on a `^` range, governed by the normal npm/Dependabot flow. A scripts/ test must not dictate the whole repo's runner version.",
  },
  {
    file: "scripts/generate-local-catalog.test.mjs",
    specifier: "vitest",
    why: "The repo's existing test runner, reached by a scripts/ test the same way 56 src/ tests reach it. It adds no new dependency — `just frontend-test` already runs this file via a vitest include glob.",
    pinExempt:
      "Exact-pinning is the right bar for a format library whose behaviour we cannot verify by hand. It is the wrong bar for the shared test runner: vitest is a repo-wide devDependency on `^`, governed by the normal npm/Dependabot flow and used by every other test in the tree. Pinning it here would let one scripts/ test dictate the whole repo's runner version.",
  },
];

// ---------------------------------------------------------------------------------------------
// Scan. Matched at STATEMENT level, not by searching the file text for a quoted specifier.
//
// The text-search version of this scanner was wrong, and wrong in an instructive way: it flagged
// this very file, because a gate that documents the syntax it matches necessarily contains that
// syntax in its own prose. Any file explaining an import rule would trip it. So:
//
//   - static imports must begin a line (ESM requires top level, and prettier formats them that
//     way here), which excludes every comment line for free;
//   - the `from` clause is then found within the same statement, bounded by the terminating `;`,
//     so a multi-line `import { … } from "node:fs"` is still caught while the match cannot run on
//     past the statement into unrelated text;
//   - dynamic `import(…)` can appear mid-expression, so it is matched per line, skipping lines
//     that are comment continuations.
//
// The remaining bias is still toward over-matching: a dynamic import written inside a trailing
// `/* … */` on a code line would be reported. That direction is safe — a false positive fails
// loudly and is one allowlist line away — whereas a missed import is a gate that passes forever
// while checking nothing.
// ---------------------------------------------------------------------------------------------

/** True for a line that is purely comment: `//…`, or a `*`/`/*` block continuation. */
const isCommentLine = (line) => /^\s*(\/\/|\*|\/\*)/.test(line);

/** Every module specifier `src` imports, static or dynamic. */
function specifiersIn(src) {
  const found = [];
  // Static: a line-initial `import`/`export`, up to that statement's `from "…"`. `[^;]*?` cannot
  // cross a statement boundary; the `s` flag lets it cross newlines within one.
  for (const m of src.matchAll(/^[ \t]*(?:import|export)\b[^;]*?\bfrom\s*["']([^"']+)["']/gms)) {
    found.push(m[1]);
  }
  // Static side-effect: a line-initial bare `import "…"`.
  for (const m of src.matchAll(/^[ \t]*import\s*["']([^"']+)["']/gm)) found.push(m[1]);
  // Dynamic, per line so comment continuations can be skipped.
  for (const line of src.split("\n")) {
    if (isCommentLine(line)) continue;
    for (const m of line.matchAll(/\bimport\s*\(\s*["']([^"']+)["']\s*\)/g)) found.push(m[1]);
  }
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
for (const { specifier, pinExempt } of ALLOWED) {
  const dev = pkg.devDependencies?.[specifier];
  const prod = pkg.dependencies?.[specifier];
  if (prod !== undefined) {
    problems.push(
      `"${specifier}" is in dependencies — a scripts/ dependency must be devDependencies only (never shipped)`,
    );
  }
  if (dev === undefined && prod === undefined) {
    problems.push(`"${specifier}" is allowed for scripts/ but is not in package.json at all`);
  } else if (dev !== undefined && !EXACT.test(dev) && !pinExempt) {
    problems.push(
      `"${specifier}" is pinned "${dev}" — a scripts/ dependency takes an exact version, no range operator (or an explicit \`pinExempt\` reason)`,
    );
  }
  // An exemption with no stated reason is not an exemption, it is an unexplained hole.
  if (pinExempt !== undefined && String(pinExempt).trim().length < 20) {
    problems.push(
      `"${specifier}" sets pinExempt without a real reason — state why the exact-pin rule does not apply`,
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
