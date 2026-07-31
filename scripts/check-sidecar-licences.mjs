// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Every Python package the sidecar installs is under a licence we have looked at and accepted.
//
// WHY. pr.yml's `dependencies` job is named for all three ecosystems. Rust had cargo-deny, npm got
// its gate with the AGPL-conveyance work, and Python had only pip-audit — which looks for CVEs and
// says nothing about terms. The locks pull ~80 packages, nearly all of them transitive, and a
// re-licence four levels down would have arrived with no one reading it.
//
// WHAT THIS IS NOT. PM does not *convey* these packages. The installers ship `requirements.lock`;
// pip fetches the wheels onto the user's own machine on first run. So this is not the attribution
// duty that `THIRD-PARTY-NOTICES.txt` discharges for the crates and npm packages that really do
// ship inside the binary — nothing here belongs in that file. The duty this gate serves is a
// plainer one: PM tells a user's machine to install these, so PM should know what they are, and a
// change should be a decision rather than a surprise.
//
// OFFLINE AND ZERO-DEPENDENCY (INVARIANTS.md I-18). It compares two committed files and reaches
// nothing. `sidecar/licences.json` is refreshed by `scripts/regen-sidecar-licences.mjs`, which is
// where the network and the human review live; the `stamp` each entry carries is what that script
// uses to notice upstream metadata moving. This gate deliberately does not re-derive it — it
// cannot, offline — so it checks the things a committed pair of files really can prove: that every
// locked package is covered, at the locked version, by a licence someone accepted.

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

import { parseEntries, normalise } from "./check-requirements-lock.mjs";
import { acceptable } from "./check-npm-licenses.mjs";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");

export const LICENCES_FILE = "sidecar/licences.json";
export const LOCKS = [
  "sidecar/requirements.lock",
  "sidecar/requirements-ocr.lock",
  "sidecar/requirements-tsne.lock",
];

/**
 * Licences accepted for the sidecar venv. Same permissive posture as deny.toml and the npm gate,
 * with the Python ecosystem's own long tail added deliberately:
 *
 *   MIT-0 / MIT-CMU  cffi and pillow — MIT variants, both more permissive than MIT itself.
 *   PSF-2.0          typing-extensions, defusedxml, parts of exceptiongroup — the Python licence.
 *   MPL-2.0          certifi (via requests) and half of tqdm. File-level copyleft: modifying an
 *                    MPL file obliges you to publish that file, which PM does not do — it installs
 *                    these unmodified. MPL-2.0 is also explicitly compatible with the GPL family
 *                    unless a file is marked "Incompatible With Secondary Licenses", and none here
 *                    is. Accepted knowingly, not by omission.
 *   LGPL-3.0-only    the libheif and libde265 shared libraries inside the pi-heif wheel, which is
 *                    how the optional photo-OCR component decodes an iPhone HEIC. Weak copyleft
 *                    reached through dynamic linking, unmodified, and — like everything else here —
 *                    not conveyed by PM. LGPL-3.0 is compatible with AGPL-3.0-or-later, which
 *                    GPL-2.0-only is NOT: that difference is exactly why the pin is pi-heif rather
 *                    than pillow-heif (see OPTIONAL_OCR_PINS in sidecar.rs).
 *
 * Adding an entry is a decision: it means PM is prepared to put that licence on a user's machine.
 * Note what is deliberately absent — no GPL of any version, and no non-commercial or
 * source-unavailable terms. A package that needs one of those is a conversation, not an edit.
 */
const ALLOWED = new Set([
  "MIT",
  "MIT-0",
  "MIT-CMU",
  "ISC",
  "Apache-2.0",
  "BSD-2-Clause",
  "BSD-3-Clause",
  "0BSD",
  "Zlib",
  "PSF-2.0",
  "MPL-2.0",
  "LGPL-3.0-only",
  "Unlicense",
  "CC0-1.0",
]);

/** Fewest packages the three locks can plausibly resolve to; below this the parse broke. */
const PACKAGE_FLOOR = 60;

/**
 * The union of the three locks as `{name, versions}`, PEP 503-normalised and sorted.
 *
 * A package legitimately has SEVERAL pins. The locks are resolved `--universal`, so one file
 * covers every platform and every CPython from MIN_PYTHON up, and where the resolution forks the
 * lock carries each fork behind an environment marker — numpy is pinned three times, once per
 * Python range. Every one of those versions gets installed on somebody's machine, so the licence
 * has to be known for all of them, not for whichever the parser happened to see first.
 *
 * The one real cross-lock invariant is the constraint: the optional locks are compiled
 * `--constraint sidecar/requirements.lock` precisely so an on-demand OCR or t-SNE install cannot
 * move a package the base venv is already running on. So an optional lock may pin FEWER versions
 * than the base (its own markers are narrower), but never a version the base lock does not have.
 */
export function lockedPackages(root, locks = LOCKS) {
  const [baseLock, ...optionalLocks] = locks;
  const perLock = new Map();
  for (const rel of locks) {
    const versions = new Map();
    for (const entry of parseEntries(readFileSync(join(root, rel), "utf8"))) {
      const name = normalise(entry.name);
      if (!versions.has(name)) versions.set(name, new Set());
      versions.get(name).add(entry.version);
    }
    perLock.set(rel, versions);
  }

  const conflicts = [];
  const base = perLock.get(baseLock);
  for (const rel of optionalLocks) {
    for (const [name, versions] of perLock.get(rel)) {
      const inBase = base.get(name);
      if (!inBase) continue; // OCR/t-SNE-only packages have no base pin to agree with.
      for (const version of versions) {
        if (!inBase.has(version)) {
          conflicts.push(
            `${rel} pins ${name} ${version}, which ${baseLock} does not pin (it has ` +
              `${[...inBase].join(", ")}) — installing that component would move a package the ` +
              `base venv is already running on, which the \`--constraint\` exists to prevent`,
          );
        }
      }
    }
  }

  const merged = new Map();
  for (const versions of perLock.values()) {
    for (const [name, set] of versions) {
      if (!merged.has(name)) merged.set(name, new Set());
      for (const v of set) merged.get(name).add(v);
    }
  }
  const packages = [...merged]
    .map(([name, set]) => ({ name, versions: [...set].sort() }))
    .sort((a, b) => (a.name < b.name ? -1 : 1));
  packages.conflicts = conflicts;
  return packages;
}

export function scan(root) {
  const locked = lockedPackages(root);
  const problems = [...locked.conflicts];
  const recorded = JSON.parse(readFileSync(join(root, LICENCES_FILE), "utf8")).packages ?? {};

  for (const { name, versions } of locked) {
    const entry = recorded[name];
    const at = versions.join(", ");
    if (!entry) {
      problems.push(
        `${name} (${at}) is in the locks but not in ${LICENCES_FILE} — run \`just lock-regen\` ` +
          `and review what it adds`,
      );
      continue;
    }
    const reviewed = entry.versions ?? [];
    const unreviewed = versions.filter((v) => !reviewed.includes(v));
    if (unreviewed.length > 0) {
      problems.push(
        `${name} is locked at ${at} but ${LICENCES_FILE} was reviewed against ` +
          `${reviewed.join(", ") || "nothing"} — ${unreviewed.join(", ")} carries no reviewed ` +
          `licence, and a release can change its terms`,
      );
      continue;
    }
    if (!entry.licence) {
      problems.push(
        `${name} (${at}) has no reviewed licence in ${LICENCES_FILE} — it is new, or its upstream ` +
          `metadata changed and the old answer was withdrawn; read its \`evidence\` and fill ` +
          `\`licence\` in`,
      );
      continue;
    }
    if (!acceptable(entry.licence, ALLOWED)) {
      problems.push(
        `${name} (${at}) is ${entry.licence}, which is not in this file's accepted set — add it ` +
          `deliberately, or drop the dependency`,
      );
    }
  }

  // Stale entries are how a licence file rots into fiction: a package leaves the locks, its row
  // stays, and the file reads as though it still describes what gets installed.
  const lockedNames = new Set(locked.map((p) => p.name));
  for (const name of Object.keys(recorded)) {
    if (!lockedNames.has(name)) {
      problems.push(
        `${LICENCES_FILE} still lists ${name}, which no longer appears in any lock — run ` +
          `\`just lock-regen\` to drop it`,
      );
    }
  }

  if (locked.length < PACKAGE_FLOOR) {
    problems.push(
      `only ${locked.length} packages found across ${LOCKS.length} locks (expected at least ` +
        `${PACKAGE_FLOOR}) — a lock is truncated, or the parser has stopped matching`,
    );
  }
  return { problems, count: locked.length };
}

function main() {
  const { problems, count } = scan(repoRoot);
  if (problems.length > 0) {
    console.error("✗ sidecar-licences:\n");
    for (const p of problems) console.error(`  • ${p}`);
    process.exit(1);
  }
  console.log(
    `✓ sidecar-licences: all ${count} Python packages across ${LOCKS.length} locks carry a ` +
      `reviewed, accepted licence`,
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}
