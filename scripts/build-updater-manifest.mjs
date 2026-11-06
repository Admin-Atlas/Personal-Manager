// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Assembles the Tauri updater manifest (`latest.json`) from the signed bundle
// artifacts produced by `tauri build`. The updater on each desktop fetches this
// file, compares `version` against the running app, and — if newer — downloads
// the matching platform `url` and verifies it against the embedded `.sig`.
//
// We assemble it ourselves from the matrix build artifacts in the publish job. Every
// download URL points at this repo's own Releases (`RELEASES_REPO` resolves to
// `github.repository` in CI), where the signed installers are attached.
//
// The manifest also states the oldest macOS the release opens on (`minimum_macos_version`,
// from tauri.conf.json), which PM checks before it downloads anything, from 3.175.0 on
// (src/lib/updateGate.ts): macOS refuses to open an app whose minimum is above it, and the
// updater replaces PM in place, so installing it would leave a PM that doesn't open.
// Tauri's updater ignores top-level fields it doesn't know, so older clients read this
// manifest exactly as before. A release that raises the minimum must come after one that
// already checks it (see RELEASING.md).
//
// Env:
//   ARTIFACTS_DIR  directory to scan recursively for installers + .sig files
//   RELEASES_REPO  owner/name of the repo whose Releases host the assets
//                  (in CI this is github.repository)
//   TAG            git tag of this release (e.g. v0.2.0)
//   VERSION        semver without leading "v" (e.g. 0.2.0)
//   NOTES          optional release notes string
//   OUT            path to write latest.json
//   TAURI_CONF     tauri.conf.json holding bundle.macOS.minimumSystemVersion
//                  (default: src-tauri/tauri.conf.json)
//
// Importing the module builds nothing (entry-point guard at the bottom).

import { readdirSync, statSync, readFileSync, writeFileSync } from "node:fs";
import { join, basename } from "node:path";
import { pathToFileURL } from "node:url";

/**
 * The platform keys the Mac bundle is listed under. Tauri's updater looks up `darwin-<the running
 * binary's arch>`, and the one universal bundle serves both Intel and Apple Silicon.
 */
export const MAC_KEYS = ["darwin-x86_64", "darwin-aarch64"];

/** The top-level field stating the oldest macOS this release opens on. */
export const MACOS_MINIMUM_FIELD = "minimum_macos_version";

/** A macOS version as tauri.conf.json states it: "14", "14.0" or "14.0.1". */
const MACOS_VERSION = /^\d{1,3}(\.\d{1,3}){0,2}$/;

/**
 * The minimum macOS tauri.conf.json sets (`bundle.macOS.minimumSystemVersion`), checked to be a
 * plain dotted version — the shape PM's update check parses, so a value it couldn't read never
 * ships.
 */
export function minimumMacosOf(conf) {
  const v = conf?.bundle?.macOS?.minimumSystemVersion;
  if (typeof v !== "string" || !MACOS_VERSION.test(v)) {
    throw new Error(
      `tauri.conf.json's bundle.macOS.minimumSystemVersion is ${JSON.stringify(v)} — expected a version like "14.0"`,
    );
  }
  return v;
}

/** Recursively collect every file path under `dir`. */
function walk(dir) {
  const out = [];
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) out.push(...walk(p));
    else out.push(p);
  }
  return out;
}

/**
 * The manifest for the artifacts under `artifactsDir`. `minimumMacos` is written beside the Mac
 * entry, and only when there is one.
 */
export function buildManifest({
  artifactsDir,
  releasesRepo,
  tag,
  version,
  notes = "",
  minimumMacos,
  pubDate,
}) {
  const files = walk(artifactsDir);

  /** The download URL for an asset once it's uploaded to the public release. */
  const urlFor = (file) =>
    `https://github.com/${releasesRepo}/releases/download/${tag}/${basename(file)}`;

  /**
   * Find the one updater artifact matching `suffix` plus its sibling `.sig`.
   * Requires EXACTLY one match: a stale re-run with leftover artifacts, or a future
   * second Windows arch, would otherwise make the readdir-ordered pick silently
   * arbitrary — so throw and turn that into a loud publish failure (audit T1-13).
   */
  function pair(suffix) {
    const matches = files.filter((f) => f.endsWith(suffix) && !f.endsWith(".sig"));
    if (matches.length === 0) return null;
    if (matches.length > 1) {
      throw new Error(
        `Ambiguous updater artifact for "${suffix}": ${matches.map((f) => basename(f)).join(", ")} — ` +
          "expected exactly one candidate",
      );
    }
    const [artifact] = matches;
    const sig = files.find((f) => f === `${artifact}.sig`);
    if (!sig) throw new Error(`Found ${basename(artifact)} but no matching .sig`);
    return { url: urlFor(artifact), signature: readFileSync(sig, "utf8").trim() };
  }

  const platforms = {};

  // Windows: NSIS `-setup.exe` is both the manual installer and the updater target.
  const win = pair("-setup.exe");
  if (win) platforms["windows-x86_64"] = win;

  // macOS: one universal `.app.tar.gz` serves both Intel and Apple Silicon.
  const mac = pair(".app.tar.gz");
  if (mac) for (const key of MAC_KEYS) platforms[key] = mac;

  // Linux: the AppImage is the only format Tauri's updater handles (the rpm is a
  // plain release asset, updated via dnf/manually).
  const linux = pair(".AppImage");
  if (linux) platforms["linux-x86_64"] = linux;

  if (Object.keys(platforms).length === 0) {
    throw new Error(`No updater artifacts found under ${artifactsDir}`);
  }

  const manifest = { version, notes, pub_date: pubDate, platforms };
  if (mac) {
    if (typeof minimumMacos !== "string" || !MACOS_VERSION.test(minimumMacos)) {
      throw new Error(`a Mac entry needs the minimum macOS, got ${JSON.stringify(minimumMacos)}`);
    }
    manifest[MACOS_MINIMUM_FIELD] = minimumMacos;
  }
  return manifest;
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const {
    ARTIFACTS_DIR,
    RELEASES_REPO,
    TAG,
    VERSION,
    NOTES = "",
    OUT,
    TAURI_CONF = "src-tauri/tauri.conf.json",
  } = process.env;
  for (const [k, v] of Object.entries({ ARTIFACTS_DIR, RELEASES_REPO, TAG, VERSION, OUT })) {
    if (!v) throw new Error(`Missing required env var: ${k}`);
  }
  const manifest = buildManifest({
    artifactsDir: ARTIFACTS_DIR,
    releasesRepo: RELEASES_REPO,
    tag: TAG,
    version: VERSION,
    notes: NOTES,
    minimumMacos: minimumMacosOf(JSON.parse(readFileSync(TAURI_CONF, "utf8"))),
    pubDate: new Date().toISOString(),
  });
  writeFileSync(OUT, JSON.stringify(manifest, null, 2));
  console.log(`Wrote ${OUT} for ${Object.keys(manifest.platforms).join(", ")}`);
}
