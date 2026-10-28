// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Builds PM's rpm in its own `tauri bundle` pass, after `tauri build` has made the AppImage and
// the deb, because the rpm needs a different version label from everything else.
//
// rpm's Version tag can't contain "-", and from Tauri CLI 2.12.3 the bundler refuses one (it
// moved to rpm-rs 0.30): `invalid version "3.155.1-alpha": contains invalid character`. Tauri
// accepts nothing but semver as the version, so Fedora's "3.155.1~alpha" is out too. The one
// separator both accept is "+": semver reads it as build metadata, rpm as an ordinary separator.
// So the rpm is labelled 3.155.1+alpha while the app, the AppImage and the deb stay 3.155.1-alpha.
// rpm compares it exactly as it compared the hyphenated label older releases carried (both split
// into 3, 155, 1, alpha), so a package install upgrades across the change.
//
// Only the label changes. The binary inside was compiled by `tauri build` and still reports
// 3.155.1-alpha, and the file keeps the name every release has used,
// PM-<version>-<release>.<arch>.rpm, so the release notes, SHA256SUMS and old links don't move.
//
// `--no-sign` skips only Tauri's updater signature: the rpm isn't an updater artifact (latest.json
// carries the AppImage alone, and a package install is told to reinstall), so that .sig was never
// used, and the release step needs no signing secret. rpm's own GPG signing is a separate switch
// (TAURI_SIGNING_RPM_KEY) that PM doesn't set.
//
// Run from anywhere after a release `tauri build` on Linux: `node scripts/bundle-rpm.mjs
// [extra tauri bundle args, e.g. --verbose]`. It builds the host's release rpm only, so it refuses
// --target and --debug, which would make Tauri write somewhere this script doesn't look. Shared by
// release.yml and linux-bundle-dryrun.yml so the two can't drift.

import { spawnSync } from "node:child_process";
import { existsSync, readdirSync, readFileSync, renameSync, statSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");

/**
 * The rpm label for a PM version: the pre-release hyphen becomes "+". Anything that wouldn't
 * come out legal for both semver and rpm (a second "-", existing build metadata, a missing
 * patch number) is refused rather than guessed at.
 */
export function rpmVersion(version) {
  const m = /^(\d+\.\d+\.\d+)(?:-([0-9A-Za-z.]+))?$/.exec(version);
  if (!m) {
    throw new Error(
      `can't label version "${version}" for the rpm: expected X.Y.Z or X.Y.Z-<pre-release> ` +
        'with no "+" and no second "-"',
    );
  }
  return m[2] ? `${m[1]}+${m[2]}` : m[1];
}

/** The [from, to] renames that give the rpm files `tauri bundle` named by `label` the release's own name. */
export function renamesFor(fileNames, version, label) {
  if (label === version) return [];
  const from = `-${label}-`;
  const to = `-${version}-`;
  return fileNames
    .filter((name) => name.endsWith(".rpm") && name.includes(from))
    .map((name) => [name, name.replace(from, to)]);
}

/**
 * The first passed-through argument that would move Tauri's output away from target/release, or
 * null. An early, plainer refusal only: pickBuilt below is what actually stops a moved rpm, or a
 * leftover one, being reported as built.
 */
export function movingArg(args) {
  for (const a of args) {
    if (/^--(target|debug)(=.*)?$/.test(a)) return a;
    if (!/^-[^-]/.test(a)) continue;
    // A cluster of short flags (-vd, -dt x, -tx86_64-...): clap reads letters until one that
    // takes a value (b, c, f or t), and the rest of the cluster is that value.
    for (const c of a.slice(1)) {
      if (c === "d" || c === "t") return a;
      if ("bcf=".includes(c)) break;
    }
  }
  return null;
}

/**
 * From the rpm folder's files ({name, mtimeMs}), the renames to make and the one rpm this run
 * built. Only files written since `started` count, so an rpm Tauri wrote somewhere else (a
 * --target, CARGO_BUILD_TARGET or a cargo [build] target moves it) fails here instead of a
 * leftover from an earlier run passing as the new build.
 */
export function pickBuilt(entries, version, label, started) {
  // Two seconds' slack for filesystems that keep coarse timestamps.
  const fresh = entries.filter((e) => e.mtimeMs >= started - 2000).map((e) => e.name);
  const renames = renamesFor(fresh, version, label);
  const after = fresh.map((name) => renames.find(([from]) => from === name)?.[1] ?? name);
  const built = after.filter((name) => name.endsWith(".rpm") && name.includes(`-${version}-`));
  if (built.length !== 1) {
    throw new Error(
      `expected one newly built rpm for ${version}, found ${built.length}` +
        (built.length ? `: ${built.join(", ")}` : "") +
        ". This script makes the host's release rpm only; --target, --debug, CARGO_BUILD_TARGET " +
        "or a cargo [build] target puts it somewhere else.",
    );
  }
  return { renames, built: built[0] };
}

/** Where cargo, and so Tauri, puts the target dir: Tauri asks `cargo metadata` from src-tauri. */
function cargoTargetDir() {
  const run = spawnSync("cargo", ["metadata", "--no-deps", "--format-version", "1"], {
    cwd: join(repoRoot, "src-tauri"),
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  if (run.error) throw run.error;
  if (run.status !== 0) throw new Error(`cargo metadata failed: ${run.stderr}`);
  return JSON.parse(run.stdout).target_directory;
}

function main() {
  const conf = JSON.parse(readFileSync(join(repoRoot, "src-tauri", "tauri.conf.json"), "utf8"));
  const version = conf.version;
  const label = rpmVersion(version);
  const moving = movingArg(process.argv.slice(2));
  if (moving) throw new Error(`bundle-rpm builds the host's release rpm only; drop ${moving}`);

  const tauri = join(repoRoot, "node_modules", ".bin", "tauri");
  const args = [
    "bundle",
    "--bundles",
    "rpm",
    "--no-sign",
    "--config",
    JSON.stringify({ version: label }),
    ...process.argv.slice(2),
  ];
  const dir = join(cargoTargetDir(), "release", "bundle", "rpm");
  console.log(`bundle-rpm: labelling the rpm ${label} (PM ${version})`);
  const started = Date.now();
  const run = spawnSync(tauri, args, { cwd: repoRoot, stdio: "inherit" });
  if (run.error) throw run.error;
  if (run.status !== 0) process.exit(run.status ?? 1);

  const entries = existsSync(dir)
    ? readdirSync(dir)
        .map((name) => ({ name, stat: statSync(join(dir, name)) }))
        .filter((e) => e.stat.isFile())
        .map((e) => ({ name: e.name, mtimeMs: e.stat.mtimeMs }))
    : [];
  const { renames, built } = pickBuilt(entries, version, label, started);
  for (const [from, to] of renames) {
    renameSync(join(dir, from), join(dir, to));
    console.log(`bundle-rpm: ${from} -> ${to}`);
  }
  console.log(`bundle-rpm: ${join(dir, built)}`);
}

// Run only when invoked directly, so the test can import the pure helpers.
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}
