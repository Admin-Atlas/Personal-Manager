// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Refreshes `sidecar/licences.json` — what licence every Python package in the sidecar's locks is
// under. NOT part of `just check`: it reaches PyPI. Run it (`just lock-regen` runs it after the
// locks) whenever the locks move, then review anything it marks and commit the result.
// `just sidecar-licences` is the offline gate that fails if you forget.
//
// IT DOES NOT DECIDE THE LICENCE. That is the whole design. PyPI's licence metadata is not
// trustworthy enough to normalise automatically, and this is not a hypothetical — across the 80
// packages in the three locks:
//
//   • pillow-heif declares `BSD-3-Clause` and simultaneously classifies itself GPLv2
//   • numpy, scipy and pandas paste their entire licence text into the one-line `license` field
//   • fsspec, tokenizers and loguru declare nothing at all and have only a classifier
//   • "BSD License" is a classifier; it does not say two-clause or three
//   • fastembed (Apache-2.0) classifies itself "Other/Proprietary License"
//
// A normaliser fed that would be guessing, and a wrong guess in a licence file reads as
// compliance. So this script only ever GATHERS: it records the raw upstream evidence, and a human
// writes the `licence` value having looked at it. What the script does own is CHANGE DETECTION —
// it stamps the evidence it saw, and when upstream's metadata moves it blanks the licence and
// hands it back for review rather than carrying the old answer forward.
//
// New or changed packages are written with `licence: null` and the run exits non-zero, so an
// unreviewed package cannot slip through quietly into a green tree.

import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { lockedPackages, LICENCES_FILE, LOCKS } from "./check-sidecar-licences.mjs";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");

/**
 * The upstream fields worth watching, in a stable shape so the stamp only moves when the licence
 * story does. The long description, upload times and file lists are deliberately not in here.
 */
export function evidenceOf(info) {
  return {
    expression: info?.license_expression ?? null,
    license: typeof info?.license === "string" ? info.license : null,
    classifiers: (info?.classifiers ?? []).filter((c) => c.startsWith("License ::")).sort(),
  };
}

export function stampOf(evidence) {
  return createHash("sha256").update(JSON.stringify(evidence), "utf8").digest("hex").slice(0, 32);
}

/** The `license` field is sometimes an entire licence text; keep the file readable. */
function display(value) {
  if (typeof value !== "string") return value;
  const flat = value.replace(/\s+/g, " ").trim();
  return flat.length > 120 ? `${flat.slice(0, 117)}...` : flat;
}

async function fetchInfo(name, version) {
  const url = `https://pypi.org/pypi/${encodeURIComponent(name)}/${encodeURIComponent(version)}/json`;
  const res = await fetch(url, { headers: { accept: "application/json" } });
  if (!res.ok) throw new Error(`${name} ${version}: PyPI returned HTTP ${res.status}`);
  return (await res.json()).info ?? {};
}

async function main() {
  const locked = lockedPackages(ROOT);
  const existing = JSON.parse(readFileSync(join(ROOT, LICENCES_FILE), "utf8"));
  const previous = existing.packages ?? {};

  const packages = {};
  const review = [];
  for (const { name, versions } of locked) {
    process.stderr.write(`sidecar-licences: ${name} ${versions.join(" ")}\n`);

    // Every pinned version, not just one. A `--universal` lock pins a package once per fork of the
    // resolution, and each of those lands on somebody's machine — asking PyPI about one of them and
    // recording the answer for all of them would be a guess wearing a fetched value's clothes.
    const perVersion = [];
    for (const version of versions) {
      perVersion.push({ version, evidence: evidenceOf(await fetchInfo(name, version)) });
    }
    const stamp = stampOf(perVersion);
    const agree = perVersion.every(
      (v) => JSON.stringify(v.evidence) === JSON.stringify(perVersion[0].evidence),
    );
    const shown = (e) => ({
      expression: e.expression,
      license: display(e.license),
      classifiers: e.classifiers,
    });
    const old = previous[name];

    // A human's answer survives only while the versions AND the evidence behind them are unchanged.
    const sameVersions = old && JSON.stringify(old.versions ?? []) === JSON.stringify(versions);
    const keep = sameVersions && old.stamp === stamp;
    const entry = {
      versions,
      licence: keep ? old.licence : null,
      stamp,
      // Where the pinned versions declare the same thing — nearly always — record it once. Where
      // they diverge, show each, because that divergence IS the thing a reviewer needs to see.
      evidence: agree
        ? shown(perVersion[0].evidence)
        : Object.fromEntries(perVersion.map((v) => [v.version, shown(v.evidence)])),
    };
    if (keep && old.note) entry.note = old.note;
    if (!keep && old) entry.previous = { versions: old.versions, licence: old.licence };
    if (entry.licence === null) review.push(name);
    packages[name] = entry;
  }

  const sorted = Object.fromEntries(
    Object.keys(packages)
      .sort()
      .map((k) => [k, packages[k]]),
  );
  writeFileSync(
    join(ROOT, LICENCES_FILE),
    `${JSON.stringify({ ...existing, packages: sorted }, null, 2)}\n`,
  );

  process.stderr.write(
    `\nsidecar-licences: ${locked.length} packages across ${LOCKS.length} locks.\n`,
  );
  if (review.length > 0) {
    process.stderr.write(
      `${review.length} need a licence written by hand (new, or upstream metadata changed):\n` +
        review.map((n) => `  - ${n}\n`).join("") +
        `\nRead each one's \`evidence\` in ${LICENCES_FILE}, check the project's own LICENSE where the\n` +
        `evidence is ambiguous, then fill in \`licence\` and add a \`note\` if it needed a judgement.\n`,
    );
    process.exitCode = 1;
    return;
  }
  process.stderr.write("Every package already has a reviewed licence. Nothing to do.\n");
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  await main();
}
