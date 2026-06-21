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
// Env:
//   ARTIFACTS_DIR  directory to scan recursively for installers + .sig files
//   RELEASES_REPO  owner/name of the repo whose Releases host the assets
//                  (in CI this is github.repository)
//   TAG            git tag of this release (e.g. v0.2.0)
//   VERSION        semver without leading "v" (e.g. 0.2.0)
//   NOTES          optional release notes string
//   OUT            path to write latest.json

import { readdirSync, statSync, readFileSync, writeFileSync } from "node:fs";
import { join, basename } from "node:path";

const { ARTIFACTS_DIR, RELEASES_REPO, TAG, VERSION, NOTES = "", OUT } = process.env;

for (const [k, v] of Object.entries({ ARTIFACTS_DIR, RELEASES_REPO, TAG, VERSION, OUT })) {
  if (!v) throw new Error(`Missing required env var: ${k}`);
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

const files = walk(ARTIFACTS_DIR);

/** The download URL for an asset once it's uploaded to the public release. */
const urlFor = (file) =>
  `https://github.com/${RELEASES_REPO}/releases/download/${TAG}/${basename(file)}`;

/** Find the updater artifact matching `suffix` plus its sibling `.sig`. */
function pair(suffix) {
  const artifact = files.find((f) => f.endsWith(suffix) && !f.endsWith(".sig"));
  if (!artifact) return null;
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
if (mac) {
  platforms["darwin-x86_64"] = mac;
  platforms["darwin-aarch64"] = mac;
}

if (Object.keys(platforms).length === 0) {
  throw new Error(`No updater artifacts found under ${ARTIFACTS_DIR}`);
}

const manifest = {
  version: VERSION,
  notes: NOTES,
  pub_date: new Date().toISOString(),
  platforms,
};

writeFileSync(OUT, JSON.stringify(manifest, null, 2));
console.log(`Wrote ${OUT} for ${Object.keys(platforms).join(", ")}`);
