// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The npm half of the third-party NOTICE, written to stdout for `just notice` to append.
//
// WHY. `cargo about` attributes the Rust crates and nothing else, so the shipped webview bundle —
// 122 production packages, including the four @fontsource families PM self-hosts — was conveyed with
// no attribution at all. MIT and ISC both require the copyright notice to travel with the software,
// and OFL-1.1 requires its own notice, so this was an unmet obligation on every release, not a
// tidiness matter.
//
// Each package's copyright line lives in its own LICENSE file, which is why this reads node_modules
// rather than working from the lockfile alone: a generic "MIT" block would satisfy nobody, because
// what MIT actually requires preserved is the copyright holder's name.
//
// It FAILS rather than emitting a short NOTICE when node_modules is missing or a licence file cannot
// be found. A NOTICE that silently omits packages is worse than none: it looks like compliance.

import { existsSync, readFileSync, readdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { productionPackages } from "./check-npm-licenses.mjs";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");

/** Filenames that hold a licence text, in the order worth trying. */
const LICENCE_FILES = [
  "LICENSE",
  "LICENSE.md",
  "LICENSE.txt",
  "LICENCE",
  "LICENCE.md",
  "LICENCE.txt",
  "LICENSE-MIT",
  "LICENSE-MIT.txt",
  "LICENSE-APACHE",
  "COPYING",
];

/** A package's licence text, or null when it ships none. */
export function licenceTextFor(packageDir) {
  for (const name of LICENCE_FILES) {
    const path = join(packageDir, name);
    if (existsSync(path)) return readFileSync(path, "utf8").replace(/\r\n/g, "\n").trim();
  }
  // Some packages put it in a subfolder, and @fontsource ships one LICENSE per family.
  if (existsSync(packageDir)) {
    for (const entry of readdirSync(packageDir)) {
      if (/^licen[cs]e/i.test(entry)) {
        const path = join(packageDir, entry);
        try {
          return readFileSync(path, "utf8").replace(/\r\n/g, "\n").trim();
        } catch {
          // A directory named "licenses" — keep looking rather than crashing.
        }
      }
    }
  }
  return null;
}

/** The NOTICE section, plus anything that could not be attributed. */
export function buildNotice(packages, readText) {
  const missing = [];
  const blocks = [];
  for (const pkg of [...packages].sort((a, b) => a.name.localeCompare(b.name))) {
    const text = readText(pkg.name);
    if (text === null) {
      missing.push(`${pkg.name}@${pkg.version} (${pkg.license ?? "no licence declared"})`);
      continue;
    }
    blocks.push(
      [
        "-".repeat(78),
        `${pkg.name} ${pkg.version}`,
        `SPDX-License-Identifier: ${pkg.license ?? "(none declared)"}`,
        "-".repeat(78),
        "",
        text,
        "",
      ].join("\n"),
    );
  }

  const header = [
    "",
    "=".repeat(78),
    "THIRD-PARTY SOFTWARE IN THE PM APPLICATION BUNDLE (npm)",
    "=".repeat(78),
    "",
    "The following packages are compiled into PM's user interface. Each is reproduced",
    "below with the licence and copyright notice its authors ship.",
    "",
    `${packages.length} packages.`,
    "",
  ].join("\n");

  return { text: header + blocks.join("\n"), missing };
}

function main() {
  const lock = JSON.parse(readFileSync(join(repoRoot, "package-lock.json"), "utf8"));
  const packages = productionPackages(lock);

  const modules = join(repoRoot, "node_modules");
  if (!existsSync(modules)) {
    console.error(
      "generate-npm-notice: node_modules is missing. Run `npm ci` first — the copyright lines this " +
        "NOTICE has to reproduce live in each package's own LICENSE file, not in the lockfile.",
    );
    process.exit(1);
  }

  const { text, missing } = buildNotice(packages, (name) => licenceTextFor(join(modules, name)));

  if (missing.length > 0) {
    console.error("generate-npm-notice: no licence text found for:\n");
    for (const m of missing) console.error(`  • ${m}`);
    console.error(
      "\nRefusing to write a NOTICE that omits them — a partial attribution file reads as compliance.",
    );
    process.exit(1);
  }

  process.stdout.write(text);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}
