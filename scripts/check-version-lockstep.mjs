// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The version-lockstep gate — the highest-value PR check. It makes the two
// recurring release bugs impossible to merge: a forgotten version bump, and
// version drift between the files that must agree.
//
// It enforces three things (the bump/tag checks are opt-in via flags so the same
// script serves local dev, the PR gate, and the release gate):
//
//   1. LOCKSTEP   — all six version-bearing files carry one identical value.
//   2. BUMPED      (--base <ref>) — that value is strictly greater, by semver,
//                  than the base branch's, so every PR moves the number.
//   3. TAG MATCH   (--tag <vX.Y.Z>) — that value equals the release tag.
//
// The matching "What's New" entry is covered for free: src/lib/changelog.ts is
// one of the six, and we read its TOP entry — so a bump with no new changelog
// entry fails lockstep (the changelog still shows the previous version).
//
// What it deliberately does NOT do: judge whether a *feature* correctly chose a
// minor bump or a *fix* a patch. That is a human call (see the versioning
// convention in the PR description / project memory); CI can only prove the
// number moved, agrees everywhere, and matches the tag.
//
// Pure Node built-ins, ESM, no dependencies — identical behaviour on every OS and
// in every caller (just / pre-commit / CI).
//
// Usage:
//   node scripts/check-version-lockstep.mjs                 # lockstep only
//   node scripts/check-version-lockstep.mjs --base origin/main   # + bumped-vs-base
//   node scripts/check-version-lockstep.mjs --tag v1.2.0-alpha   # + equals tag (release)

import { readFileSync } from "node:fs";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
// Normalise CRLF — lockfiles/manifests are CRLF on Windows checkouts, and the
// block/line regexes below assume "\n".
const read = (rel) => readFileSync(join(repoRoot, rel), "utf8").replace(/\r\n/g, "\n");

// ---- per-file extractors -------------------------------------------------
// Each returns the version string carried by that file. They are intentionally
// strict: a structural change that hides the version should fail loudly here
// rather than silently read `undefined`.

const required = (value, where) => {
  if (!value) throw new Error(`Could not find a version in ${where}`);
  return value;
};

function fromJson(rel, pick) {
  const data = JSON.parse(read(rel));
  return required(pick(data), rel);
}

/** The `version = "..."` inside Cargo.toml's `[package]` table (not a dep's). */
function fromCargoToml(rel) {
  const pkg = read(rel).match(/\[package\]([\s\S]*?)(?:\n\[|$)/);
  const m = pkg && pkg[1].match(/^\s*version\s*=\s*"([^"]+)"/m);
  return required(m && m[1], `${rel} [package] version`);
}

/** The `version` of the `[[package]] name = "pm"` entry in Cargo.lock. */
function fromCargoLock(rel) {
  for (const block of read(rel)
    .split(/\n\[\[package\]\]\n/)
    .slice(1)) {
    if (/^name = "pm"$/m.test(block)) {
      const m = block.match(/^version = "([^"]+)"/m);
      return required(m && m[1], `${rel} (pm package)`);
    }
  }
  throw new Error(`Could not find the 'pm' package entry in ${rel}`);
}

/** The TOP entry's `version` in the in-app changelog (the "What's New" view). */
function fromChangelog(rel) {
  const txt = read(rel);
  const after = txt.slice(txt.indexOf("export const CHANGELOG"));
  const m = after.match(/version:\s*"([^"]+)"/);
  return required(m && m[1], `${rel} (newest CHANGELOG entry)`);
}

const SOURCES = [
  { label: "package.json", get: () => fromJson("package.json", (d) => d.version) },
  { label: "package-lock.json (root)", get: () => fromJson("package-lock.json", (d) => d.version) },
  {
    label: 'package-lock.json (packages[""])',
    get: () => fromJson("package-lock.json", (d) => d.packages?.[""]?.version),
  },
  {
    label: "src-tauri/tauri.conf.json",
    get: () => fromJson("src-tauri/tauri.conf.json", (d) => d.version),
  },
  { label: "src-tauri/Cargo.toml", get: () => fromCargoToml("src-tauri/Cargo.toml") },
  { label: "src-tauri/Cargo.lock", get: () => fromCargoLock("src-tauri/Cargo.lock") },
  {
    label: "src/lib/changelog.ts (newest entry)",
    get: () => fromChangelog("src/lib/changelog.ts"),
  },
];

// ---- semver compare (enough for our `X.Y.Z[-pre]` scheme) ----------------

function parseSemver(v) {
  const dash = v.indexOf("-");
  const core = dash === -1 ? v : v.slice(0, dash);
  const pre = dash === -1 ? "" : v.slice(dash + 1);
  const [maj, min, pat] = core.split(".").map(Number);
  if ([maj, min, pat].some((n) => !Number.isInteger(n))) {
    throw new Error(`Not a valid X.Y.Z version: "${v}"`);
  }
  return { maj, min, pat, pre };
}

/** -1 if a<b, 0 if equal, 1 if a>b. A release outranks its own pre-release. */
function semverCmp(a, b) {
  const A = parseSemver(a);
  const B = parseSemver(b);
  for (const k of ["maj", "min", "pat"]) {
    if (A[k] !== B[k]) return A[k] < B[k] ? -1 : 1;
  }
  if (A.pre === B.pre) return 0;
  if (A.pre === "") return 1;
  if (B.pre === "") return -1;
  const ai = A.pre.split(".");
  const bi = B.pre.split(".");
  for (let i = 0; i < Math.max(ai.length, bi.length); i++) {
    if (ai[i] === undefined) return -1;
    if (bi[i] === undefined) return 1;
    const an = /^\d+$/.test(ai[i]);
    const bn = /^\d+$/.test(bi[i]);
    if (an && bn) {
      const d = Number(ai[i]) - Number(bi[i]);
      if (d !== 0) return d < 0 ? -1 : 1;
    } else if (an !== bn) {
      return an ? -1 : 1; // numeric identifiers rank below alphanumeric
    } else if (ai[i] !== bi[i]) {
      return ai[i] < bi[i] ? -1 : 1;
    }
  }
  return 0;
}

// ---- args ----------------------------------------------------------------

function argValue(name) {
  const i = process.argv.indexOf(name);
  return i !== -1 ? process.argv[i + 1] : undefined;
}
const baseRef = argValue("--base");
const tag = argValue("--tag");

// ---- run -----------------------------------------------------------------

const fail = (msg) => {
  console.error(`\n✗ version-lockstep: ${msg}`);
  process.exit(1);
};

let versions;
try {
  versions = SOURCES.map((s) => ({ label: s.label, version: s.get() }));
} catch (err) {
  fail(err.message);
}

console.log("Version across the lockstep set:");
for (const { label, version } of versions) {
  console.log(`  ${version.padEnd(16)} ${label}`);
}

// 1. LOCKSTEP
const distinct = [...new Set(versions.map((v) => v.version))];
if (distinct.length !== 1) {
  fail(
    `files disagree on the version — found ${distinct.map((v) => `"${v}"`).join(", ")}. ` +
      `Bump all of them together (regenerate package-lock.json + Cargo.lock).`,
  );
}
const current = distinct[0];
console.log(`\n✓ lockstep: all ${versions.length} files agree on ${current}`);

// 2. BUMPED vs base branch
if (baseRef) {
  let basePkg;
  try {
    basePkg = execFileSync("git", ["show", `${baseRef}:package.json`], {
      encoding: "utf8",
      cwd: repoRoot,
    });
  } catch {
    fail(
      `could not read package.json at "${baseRef}" — ensure the base branch is fetched ` +
        `(e.g. \`git fetch origin main\`).`,
    );
  }
  const baseVersion = JSON.parse(basePkg).version;
  if (semverCmp(current, baseVersion) <= 0) {
    fail(
      `version was not bumped — base ${baseRef} is ${baseVersion}, this branch is ${current}. ` +
        `Every PR must bump the version (feature → minor, fix/chore → patch) and add a What's New entry.`,
    );
  }
  console.log(`✓ bumped: ${current} > ${baseVersion} (${baseRef})`);
}

// 3. TAG MATCH (release)
if (tag) {
  const tagVersion = tag.replace(/^v/, "");
  if (current !== tagVersion) {
    fail(
      `version ${current} does not match the release tag ${tag} (expected ${tagVersion}). ` +
        `Tag and tree must agree at release.`,
    );
  }
  console.log(`✓ tag: ${current} matches ${tag}`);
}

console.log("");
