// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Every Python package the sidecar installs is under a licence we have looked at and accepted.
//
// WHY. pr.yml's `dependencies` job is named for all three ecosystems. Rust had cargo-deny, npm got
// its gate with the AGPL-conveyance work, and Python had only pip-audit — which looks for CVEs and
// says nothing about terms. The locks pull ~70 packages, nearly all of them transitive, and a
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
 * Licences accepted for the sidecar venv: the permissive base deny.toml and the npm gate share, plus,
 * deliberately, the weak copyleft below (MPL-2.0 and the LGPL family) and the Python ecosystem's own
 * permissive long tail. An entry in sidecar/licences.json
 * records the wheel AS INSTALLED: the package's own code AND every native library its wheels bundle
 * on Windows, macOS and Linux (auditwheel's `<pkg>.libs/`, delocate's `.dylibs/`, delvewheel's DLLs,
 * and code linked in statically), because that is what lands on a user's machine. Where a bundled
 * library offers a choice (`A OR B`), the entry records the side PM relies on and its note says so.
 *
 *   MIT-0 / MIT-CMU  cffi, pillow, and lxml's ElementTree-derived code — MIT variants, both more
 *                    permissive than MIT itself.
 *   PSF-2.0          typing-extensions, defusedxml, parts of exceptiongroup — the Python licence.
 *   MPL-2.0          certifi (via requests) and half of tqdm. File-level copyleft: modifying an
 *                    MPL file obliges you to publish that file, which PM does not do — it installs
 *                    these unmodified. MPL-2.0 is also explicitly compatible with the GPL family
 *                    unless a file is marked "Incompatible With Secondary Licenses", and none here
 *                    is. Accepted knowingly, not by omission.
 *   LGPL-2.1-only, LGPL-2.1-or-later, LGPL-2.0-or-later, LGPL-3.0-only, LGPL-3.0-or-later
 *                    weak copyleft in libraries the wheels bundle: GEOS in shapely (2.1 only),
 *                    libquadmath in numpy and scipy, GNU libiconv linked statically into lxml,
 *                    libheif and libde265 in pi-heif, FFmpeg and LAME in OpenCV. All unmodified and
 *                    never conveyed by PM: pip fetches the wheels from PyPI onto the user's machine,
 *                    so the distribution duties (source, licence text, relinking for a static link)
 *                    sit with the wheel's publisher. LGPL-2.1 section 3 lets anyone apply GPL-2.0
 *                    or any later GPL instead, and LGPL-3.0 is GPL-3.0 plus permissions, so both are
 *                    compatible with AGPL-3.0-or-later. GPL-2.0-only is NOT: that difference is
 *                    exactly why the pin is pi-heif rather than pillow-heif (see OPTIONAL_OCR_PINS
 *                    in sidecar.rs). Accepted 2026-11-06 (Bobby: "souds godo for lgpl, you can add
 *                    it. should we use the new version as well? i dont mind").
 *   GPL-3.0-or-later WITH GCC-exception-3.1
 *                    the GCC runtime (libgfortran, libgcc_s, libgomp, libstdc++) that numpy, scipy,
 *                    scikit-learn, openTSNE, onnxruntime and pi-heif ship because they were compiled
 *                    with GCC. The exception exists so that any program, under any licence, may
 *                    use it: it is not GPL's copyleft.
 *   the permissive long tail, in bundled libraries: BSL-1.0 (Boost), Unicode-3.0 and ICU (ICU and
 *                    Unicode data), IJG (libjpeg), libpng-2.0, libtiff, FTL (FreeType, taken over its
 *                    GPL-2.0 alternative), TCL (Tcl/Tk stubs, ujson), curl, blessing (SQLite),
 *                    MIT-Modern-Variant, MIT-open-group and X11 (HarfBuzz, X.org), NCSA (LLVM's
 *                    OpenMP), Qhull, BSD-3-Clause-Open-MPI (LAPACK), BSD-3-Clause-LBNL,
 *                    BSD-3-Clause-Clear (SVT-AV1), BSD-2-Clause-Patent (libvmaf), Apache-2.0 WITH
 *                    LLVM-exception (libc++), CC-BY-4.0 (pypdfium2's documentation), WTFPL (zimg),
 *                    LicenseRef-AGG-2.3 (Anti-Grain Geometry 2.3: copy, use, modify, sell and
 *                    distribute, keeping the notice), bzip2-1.0.6 (bzip2, linked statically into
 *                    the FreeType in pillow's Linux wheel: BSD-style, with an altered version to be
 *                    marked as such).
 *
 * Adding an entry is a decision: it means PM is prepared to put that licence on a user's machine.
 * Note what is deliberately absent — no GPL of any version outside the GCC runtime's exception, and
 * no non-commercial or source-unavailable terms. A package that needs one of those is a
 * conversation, not an edit, and if the answer is yes it goes in PACKAGE_EXCEPTIONS for that one
 * package, with the reason, never in this set.
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
  "LGPL-2.0-or-later",
  "LGPL-2.1-only",
  "LGPL-2.1-or-later",
  "LGPL-3.0-only",
  "LGPL-3.0-or-later",
  "GPL-3.0-or-later WITH GCC-exception-3.1",
  "Unlicense",
  "CC0-1.0",
  "BSL-1.0",
  "Unicode-3.0",
  "ICU",
  "IJG",
  "libpng-2.0",
  "libtiff",
  "FTL",
  "TCL",
  "curl",
  "blessing",
  "MIT-Modern-Variant",
  "MIT-open-group",
  "X11",
  "NCSA",
  "Qhull",
  "BSD-3-Clause-Open-MPI",
  "BSD-3-Clause-LBNL",
  "BSD-3-Clause-Clear",
  "BSD-2-Clause-Patent",
  "Apache-2.0 WITH LLVM-exception",
  "CC-BY-4.0",
  "WTFPL",
  "LicenseRef-AGG-2.3",
  "bzip2-1.0.6",
]);

/**
 * Operating-system runtimes a wheel carries so it runs on a machine that lacks them: accepted for
 * every package. Closed-source but freely redistributable, and part of the platform rather than of
 * any library: Microsoft's Visual C++ runtime. CPython itself is built against it and loads its C
 * part, vcruntime140, which python.org's Windows builds ship, and PM's Windows build depends on it
 * already; the msvcp140 and vcomp140 the wheels carry are the same redistributable's C++ and OpenMP
 * parts, under the same terms. Accepted 2026-11-06 (Bobby chose "Accept as system runtime").
 * Nothing that is a library in its own right belongs here.
 */
const SYSTEM_RUNTIMES = new Set(["LicenseRef-Microsoft-VC-Redistributable"]);

/**
 * Licences accepted for ONE package only, each a decision with its reason. The gate fails an
 * exception its package no longer needs, so none outlives the reason it was granted.
 */
export const PACKAGE_EXCEPTIONS = {
  // Photo OCR's image library, reached only through rapidocr for pixel work (colour conversion,
  // resizing, contours): none of the video, network or GUI code below is reachable from PM. Its
  // macOS arm64 wheel bundles Homebrew's FFmpeg built with --enable-gpl, with x264, x265,
  // rubberband and vidstab (GPL-2.0-or-later; GPL-3.0-or-later as a whole); every x86_64 wheel
  // links Intel's IPP ICV statically (binary-only terms); the Linux wheel carries OpenSSL 1.1.1
  // (the old OpenSSL licence, and end-of-life upstream). Accepted for now, 2026-11-06 (Bobby chose
  // "Accept now, look for a swap"); a board card looks for photo OCR without OpenCV.
  //
  // OFL-1.1 was found after that decision and is allowed here under it, for the same package and
  // the same swap: OpenCV 5 compiles its built-in Rubik font (SIL Open Font License 1.1) into
  // every wheel, for putText, which PM never calls. The OFL lets the font be used, embedded and
  // redistributed with any software; what it forbids is selling the font on its own, and releasing
  // a modified font under any other licence.
  "opencv-python-headless": [
    "GPL-3.0-or-later",
    "GPL-2.0-or-later",
    "LicenseRef-Intel-ISSL",
    "OpenSSL",
    "OFL-1.1",
  ],
};

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

export function scan(root, exceptions = PACKAGE_EXCEPTIONS) {
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
          `metadata changed and the old answer was withdrawn (its \`previous\` keeps it). Read ` +
          `its \`evidence\` and the project's LICENSE; for a compiled package the licence must ` +
          `also cover what its win_amd64, macOS and manylinux x86_64 wheels bundle (auditwheel's ` +
          `.libs/, delocate's .dylibs/, delvewheel's DLLs, static links), read from the wheels ` +
          `whose hashes are in the locks and compared against \`previous\`. Then fill \`licence\` ` +
          `and \`note\` in`,
      );
      continue;
    }
    const here = new Set([...ALLOWED, ...SYSTEM_RUNTIMES, ...(exceptions[name] ?? [])]);
    if (!acceptable(entry.licence, here)) {
      problems.push(
        `${name} (${at}) is ${entry.licence}, which is not in this file's accepted set — add it ` +
          `deliberately, or drop the dependency`,
      );
    }
  }

  // An exception outlives its reason the day the package drops the library that needed it.
  for (const [name, licences] of Object.entries(exceptions)) {
    const entry = recorded[name];
    if (!locked.some((p) => p.name === name)) {
      problems.push(`PACKAGE_EXCEPTIONS lists ${name}, which no lock installs any more — drop it`);
      continue;
    }
    if (!entry) {
      // The package is still installed, so the exception may well still be needed: what is
      // missing is the row it qualifies, which the loop above has already reported.
      problems.push(
        `PACKAGE_EXCEPTIONS lists ${name}, which the locks still install but ${LICENCES_FILE} ` +
          `has no row for — restore its row rather than dropping the exception`,
      );
      continue;
    }
    // A blank licence is waiting for review (reported above); it names nothing yet, so it cannot
    // say an exception is no longer needed.
    if (!entry.licence) continue;
    const parts = new Set(
      (entry.licence ?? "")
        .replace(/[()]/g, " ")
        .split(/\bAND\b|\bOR\b/)
        .map((p) => p.trim()),
    );
    for (const licence of licences) {
      if (!parts.has(licence)) {
        problems.push(
          `PACKAGE_EXCEPTIONS lets ${name} use ${licence}, which its licence no longer names — ` +
            `drop the exception`,
        );
      }
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
