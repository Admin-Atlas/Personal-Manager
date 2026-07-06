// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// CI-membership gate (audit T-04). The justfile is the declared single source of
// truth for checks, but pr.yml RE-LISTS the members of `just check` as individual
// steps rather than consuming the aggregate — so the "local and CI can't drift"
// claim holds for command CONTENT but not for COVERAGE: a recipe added to `check`
// (as sidecar-test / zizmor / pip-audit each were, historically) silently doesn't
// reach CI until pr.yml is separately edited. Nothing asserts the two agree.
//
// This closes that: expand `check` (via `check-fast`) to its leaf recipes and assert
// each is invoked as a `just <recipe>` step somewhere in pr.yml — with one mapped
// exception (cargo-deny runs via its pinned Action, not `just deny`). Pure Node.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const read = (rel) => readFileSync(join(repoRoot, rel), "utf8").replace(/\r\n/g, "\n");

const justfile = read("justfile");
const prYml = read(".github/workflows/pr.yml");

// The space-separated prerequisite list after `recipe:` (justfile dependency syntax).
function recipeDeps(name) {
  const m = justfile.match(new RegExp(`^${name}\\s*:([^\\n]*)`, "m"));
  if (!m) throw new Error(`justfile has no '${name}:' recipe`);
  return m[1].trim().split(/\s+/).filter(Boolean);
}

// Expand `check` to its leaf recipes: `check` includes the `check-fast` aggregate,
// which pr.yml runs as its individual members (there is no `just check-fast` step).
const fast = recipeDeps("check-fast");
const leaves = new Set();
for (const dep of recipeDeps("check")) {
  if (dep === "check-fast") for (const f of fast) leaves.add(f);
  else leaves.add(dep);
}

// Every recipe pr.yml invokes as `just <recipe>` (comments stripped so a recipe named
// only inside a comment can't count as covered — that would be a false pass).
const prClean = prYml.replace(/(^|\s)#[^\n]*/g, "");
const invoked = new Set([...prClean.matchAll(/\bjust\s+([a-z0-9-]+)/g)].map((m) => m[1]));

// Recipes deliberately run by a pinned Action instead of `just <recipe>`.
const COVERED_BY_ACTION = {
  deny: /EmbarkStudios\/cargo-deny-action/, // supply-chain check runs via the action
};

const problems = [];
for (const recipe of [...leaves].sort()) {
  if (invoked.has(recipe)) continue;
  const action = COVERED_BY_ACTION[recipe];
  if (action && action.test(prYml)) continue;
  problems.push(recipe);
}

if (problems.length) {
  console.error("✗ CI membership: `just check` recipes not wired into pr.yml:\n");
  for (const r of problems) console.error(`  • ${r}  — add a "just ${r}" step to pr.yml`);
  console.error("\n  Every gate member must run in CI, or a green PR skips it silently.");
  process.exit(1);
}

console.log(`✓ CI membership: all ${leaves.size} \`just check\` recipes are wired into pr.yml`);
