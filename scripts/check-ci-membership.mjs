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
//
// SECOND DIRECTION (2026-07-29 audit, batch A1). The justfile also claims `check-fast`
// is "what pre-commit runs", and that claim had rotted the same way for the same reason:
// .pre-commit-config.yaml re-lists the members by hand, so it had drifted to 9 of the 13.
// The four it had lost were licence-subset, ci-membership, sync-set and script-deps — the
// drift guards, whose entire job is to notice two files disagreeing, and which therefore
// could not notice their own absence. Every `check-fast` member must now have a hook.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const read = (rel) => readFileSync(join(repoRoot, rel), "utf8").replace(/\r\n/g, "\n");

const justfile = read("justfile");
const prYml = read(".github/workflows/pr.yml");
const preCommit = read(".pre-commit-config.yaml");

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

// --- direction 2: check-fast ⊆ pre-commit --------------------------------------------------
// Hooks invoke recipes as `entry: just <recipe>`, one per hook.
const hooked = new Set(
  [...preCommit.matchAll(/^\s*entry:\s*just\s+([a-z0-9-]+)/gm)].map((m) => m[1]),
);

// Recipes pre-commit runs through a different, deliberately-named variant.
const PRE_COMMIT_ALIAS = {
  // `version-local` is the offline-tolerant form: it does the bumped-vs-base check against
  // origin/main when that ref is fetched and degrades to lockstep-only (with a warning) when it
  // is not, so committing on a plane isn't blocked. CI runs the strict `version` (T1-10).
  version: "version-local",
};

const missing = [];
for (const recipe of [...fast].sort()) {
  if (hooked.has(recipe)) continue;
  const alias = PRE_COMMIT_ALIAS[recipe];
  if (alias && hooked.has(alias)) continue;
  missing.push(recipe);
}

// The floor: if the hook parser found almost nothing, every check above is vacuously true.
if (hooked.size < fast.length) {
  // Only a real diagnostic when it is ALSO reporting misses — a parse of 0 with 0 misses is
  // impossible, so this can't mask a broken parser.
  if (!missing.length) {
    console.error(
      `✗ CI membership: parsed only ${hooked.size} \`entry: just …\` hooks from ` +
        `.pre-commit-config.yaml but reported no misses — the hook parser is broken.`,
    );
    process.exit(1);
  }
}

if (missing.length) {
  console.error("✗ CI membership: `check-fast` recipes with no pre-commit hook:\n");
  for (const r of missing) console.error(`  • ${r}  — add a hook running "just ${r}"`);
  console.error(
    "\n  The justfile states that check-fast is what pre-commit runs. Either add the hook or\n" +
      "  stop claiming it — a subset that drifts is worse than an honest one.",
  );
  process.exit(1);
}

console.log(
  `✓ CI membership: all ${leaves.size} \`just check\` recipes are wired into pr.yml, ` +
    `and all ${fast.length} \`check-fast\` recipes have a pre-commit hook`,
);
