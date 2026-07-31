// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Every npm package that SHIPS carries a licence we have accepted.
//
// WHY. pr.yml's `dependencies` job is named for all three ecosystems and gated only Rust: `cargo
// deny` proved the crate tree, and nothing at all looked at the 122 production npm packages that end
// up inside the shipped webview bundle. A copyleft package could have arrived transitively — through
// a patch bump of something four levels down — and the first anyone would have heard of it is a
// licence complaint about a released binary.
//
// This is the npm half of `check-license-subset.mjs`'s job for Rust, and it deliberately mirrors
// deny.toml's shape: one allow-list, in one file, that a new licence must be added to on purpose.
//
// OFFLINE AND ZERO-DEPENDENCY (INVARIANTS.md I-18). `package-lock.json` records a `license` for every
// package and marks dev-only ones `dev: true`, so the whole check is a read of a committed file — no
// `npm ci`, no network, and it runs in the same hygiene job that has no node_modules.
//
// Production only. Vite bundles what `src/` imports; devDependencies build the app and are not in it.
// A dev-only licence is a question about this repo's own tooling, not about what users receive.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");

/**
 * Licences accepted for anything that ships. Permissive and attribution-only, matching the posture
 * deny.toml takes for crates. Adding an entry is a decision: it means PM is prepared to satisfy that
 * licence's terms in a distributed binary.
 *
 * OFL-1.1 is here for the four @fontsource families PM self-hosts. It permits bundling in software;
 * what it forbids is selling the fonts on their own, which PM does not do.
 */
const ALLOWED = new Set([
  "MIT",
  "ISC",
  "Apache-2.0",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "0BSD",
  "CC0-1.0",
  "Unlicense",
  "OFL-1.1",
  "BlueOak-1.0.0",
  "Python-2.0",
]);

/** Fewest production packages that can plausibly be in the tree; below this the parse broke. */
const PACKAGE_FLOOR = 50;

/**
 * Split an SPDX expression into the licences a distributor could choose to satisfy.
 *
 * `A OR B` is satisfied by either, so it passes if ANY side is allowed. `A AND B` requires both, so
 * every side must be allowed. Mixed expressions are refused rather than guessed at — a wrong guess
 * here reads as compliance.
 */
export function acceptable(expression, allowed = ALLOWED) {
  if (!expression) return false;
  const clean = expression.replace(/[()]/g, " ").trim();
  const hasOr = /\bOR\b/.test(clean);
  const hasAnd = /\bAND\b/.test(clean);
  if (hasOr && hasAnd) return false;
  const parts = clean
    .split(/\bOR\b|\bAND\b/)
    .map((p) => p.trim())
    .filter(Boolean);
  if (parts.length === 0) return false;
  return hasOr ? parts.some((p) => allowed.has(p)) : parts.every((p) => allowed.has(p));
}

/** Production (non-dev) packages from a parsed package-lock, as `{name, version, license}`. */
export function productionPackages(lock) {
  const out = [];
  for (const [path, entry] of Object.entries(lock.packages ?? {})) {
    if (!path.startsWith("node_modules/")) continue;
    if (entry.dev || entry.devOptional) continue;
    out.push({
      // The LAST node_modules segment is the package: `node_modules/a/node_modules/b` is b.
      name: path.slice(path.lastIndexOf("node_modules/") + "node_modules/".length),
      version: entry.version ?? "(no version)",
      license: entry.license ?? null,
    });
  }
  return out;
}

export function scan(root) {
  const lock = JSON.parse(readFileSync(join(root, "package-lock.json"), "utf8"));
  const packages = productionPackages(lock);
  const problems = [];

  for (const pkg of packages) {
    if (pkg.license === null) {
      problems.push(
        `${pkg.name}@${pkg.version} declares no licence in package-lock.json — it ships inside the ` +
          `webview bundle, so its terms have to be known`,
      );
    } else if (!acceptable(pkg.license)) {
      problems.push(
        `${pkg.name}@${pkg.version} is ${pkg.license}, which is not in this file's accepted set — ` +
          `add it deliberately, or remove the dependency`,
      );
    }
  }

  if (packages.length < PACKAGE_FLOOR) {
    problems.push(
      `only ${packages.length} production packages found (expected at least ${PACKAGE_FLOOR}) — the ` +
        `lockfile is truncated, or this parser has stopped matching`,
    );
  }
  return { problems, count: packages.length };
}

function main() {
  const { problems, count } = scan(repoRoot);
  if (problems.length > 0) {
    console.error("✗ npm-licenses:\n");
    for (const p of problems) console.error(`  • ${p}`);
    process.exit(1);
  }
  console.log(`✓ npm-licenses: all ${count} shipped npm packages carry an accepted licence`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}
