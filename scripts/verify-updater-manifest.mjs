// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Release safety check: prove the assembled updater manifest (latest.json) is
// internally consistent before it is published — every desktop trusts this file
// to drive auto-updates, so a malformed or mis-targeted one is a serious bug.
//
// Asserts:
//   * manifest.version equals the release VERSION (and so, transitively, the tag),
//   * at least one platform is present, and every platform is one PM ships:
//     windows-x86_64, darwin-x86_64, darwin-aarch64, linux-x86_64,
//   * every platform entry has a non-empty url AND signature,
//   * every download url points at this release's own tag on the releases repo
//     (no cross-repo / cross-tag asset can sneak in),
//   * a manifest with a Mac entry (either darwin key) states the oldest macOS the
//     release opens on (`minimum_macos_version`), equal to tauri.conf.json's
//     minimumSystemVersion, and one without a Mac entry doesn't. PM reads that field
//     before it downloads an update (src/lib/updateGate.ts), so a missing or wrong one
//     would let a Mac install a build that macOS then refuses to open.
//
// The expectations are written out here rather than imported from
// build-updater-manifest.mjs, so a mistake in the builder can't also become the rule
// it is checked against.
//
// Env: OUT (path to latest.json), VERSION (no leading v), TAG, RELEASES_REPO,
//      TAURI_CONF (default src-tauri/tauri.conf.json).
// Pure Node built-ins. Importing the module checks nothing (entry-point guard at the bottom).

import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

/** The keys the Mac bundle is listed under: one universal bundle, Intel and Apple Silicon. */
const MAC_KEYS = ["darwin-x86_64", "darwin-aarch64"];

/** The platform keys PM ships an update for. */
const SHIPPED = new Set(["windows-x86_64", ...MAC_KEYS, "linux-x86_64"]);

/** A macOS version as tauri.conf.json states it, and as PM's update check parses it. */
const MACOS_VERSION = /^\d{1,3}(\.\d{1,3}){0,2}$/;

/** Every problem with `manifest`, or an empty list when it may be published. */
export function manifestProblems({ manifest, version, tag, releasesRepo, minimumMacos }) {
  const problems = [];

  if (manifest?.version !== version) {
    problems.push(`manifest.version "${manifest?.version}" ≠ release version "${version}"`);
  }

  const platforms = manifest?.platforms ?? {};
  const names = Object.keys(platforms);
  if (names.length === 0) problems.push("no platforms in the manifest");

  const expectedPrefix = `https://github.com/${releasesRepo}/releases/download/${tag}/`;
  for (const name of names) {
    if (!SHIPPED.has(name)) {
      problems.push(`${name}: not a platform PM ships (${[...SHIPPED].join(", ")})`);
    }
    const p = platforms[name];
    if (!p?.url) problems.push(`${name}: missing url`);
    if (!p?.signature) problems.push(`${name}: missing signature`);
    if (p?.url && !p.url.startsWith(expectedPrefix)) {
      problems.push(
        `${name}: url does not point at this release — ${p.url} (expected ${expectedPrefix}…)`,
      );
    }
  }

  const stated = manifest?.minimum_macos_version;
  if (names.some((name) => MAC_KEYS.includes(name))) {
    if (typeof minimumMacos !== "string" || !MACOS_VERSION.test(minimumMacos)) {
      problems.push(
        `tauri.conf.json's minimumSystemVersion is ${JSON.stringify(minimumMacos)} — expected a version like "14.0"`,
      );
    } else if (stated !== minimumMacos) {
      problems.push(
        `minimum_macos_version is ${JSON.stringify(stated)}, but tauri.conf.json's minimumSystemVersion is "${minimumMacos}"`,
      );
    }
  } else if (stated !== undefined) {
    problems.push("minimum_macos_version is set, but the manifest has no Mac entry");
  }

  return problems;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const {
    OUT,
    VERSION,
    TAG,
    RELEASES_REPO,
    TAURI_CONF = "src-tauri/tauri.conf.json",
  } = process.env;
  for (const [k, v] of Object.entries({ OUT, VERSION, TAG, RELEASES_REPO })) {
    if (!v) throw new Error(`Missing required env var: ${k}`);
  }

  const manifest = JSON.parse(readFileSync(OUT, "utf8"));
  const conf = JSON.parse(readFileSync(TAURI_CONF, "utf8"));
  const problems = manifestProblems({
    manifest,
    version: VERSION,
    tag: TAG,
    releasesRepo: RELEASES_REPO,
    minimumMacos: conf?.bundle?.macOS?.minimumSystemVersion,
  });

  if (problems.length) {
    console.error("✗ updater manifest is not internally consistent:\n");
    for (const p of problems) console.error(`  • ${p}`);
    process.exit(1);
  }

  const names = Object.keys(manifest.platforms);
  const mac = manifest.minimum_macos_version
    ? ` (macOS ${manifest.minimum_macos_version} or newer)`
    : "";
  console.log(`✓ updater manifest ok: ${VERSION} → ${names.join(", ")}${mac}`);
}
