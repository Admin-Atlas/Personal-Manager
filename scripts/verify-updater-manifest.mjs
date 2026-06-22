// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Release safety check: prove the assembled updater manifest (latest.json) is
// internally consistent before it is published — every desktop trusts this file
// to drive auto-updates, so a malformed or mis-targeted one is a serious bug.
//
// Asserts:
//   * manifest.version equals the release VERSION (and so, transitively, the tag),
//   * at least one platform is present,
//   * every platform entry has a non-empty url AND signature,
//   * every download url points at this release's own tag on the releases repo
//     (no cross-repo / cross-tag asset can sneak in).
//
// Env: OUT (path to latest.json), VERSION (no leading v), TAG, RELEASES_REPO.
// Pure Node built-ins.

import { readFileSync } from "node:fs";

const { OUT, VERSION, TAG, RELEASES_REPO } = process.env;
for (const [k, v] of Object.entries({ OUT, VERSION, TAG, RELEASES_REPO })) {
  if (!v) throw new Error(`Missing required env var: ${k}`);
}

const manifest = JSON.parse(readFileSync(OUT, "utf8"));
const problems = [];

if (manifest.version !== VERSION) {
  problems.push(`manifest.version "${manifest.version}" ≠ release version "${VERSION}"`);
}

const platforms = manifest.platforms ?? {};
const names = Object.keys(platforms);
if (names.length === 0) problems.push("no platforms in the manifest");

const expectedPrefix = `https://github.com/${RELEASES_REPO}/releases/download/${TAG}/`;
for (const name of names) {
  const p = platforms[name];
  if (!p?.url) problems.push(`${name}: missing url`);
  if (!p?.signature) problems.push(`${name}: missing signature`);
  if (p?.url && !p.url.startsWith(expectedPrefix)) {
    problems.push(
      `${name}: url does not point at this release — ${p.url} (expected ${expectedPrefix}…)`,
    );
  }
}

if (problems.length) {
  console.error("✗ updater manifest is not internally consistent:\n");
  for (const p of problems) console.error(`  • ${p}`);
  process.exit(1);
}

console.log(`✓ updater manifest ok: ${VERSION} → ${names.join(", ")}`);
