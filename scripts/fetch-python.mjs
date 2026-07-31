// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Fetch the bundled, relocatable CPython for the packaged app (spec §4.6).
//
// PM's document sidecar needs Python. Rather than require the user to install
// it, the Windows and Linux builds SHIP a standalone interpreter (from python-
// build-standalone — the same relocatable builds `uv` uses) as a Tauri resource,
// and provision the managed venv against it at first ingest. This script
// downloads that interpreter at BUILD time and unpacks it to `src-tauri/python/`,
// which `tauri.windows.conf.json` / `tauri.linux.conf.json` then bundle into the
// NSIS installer / AppImage+rpm.
//
// It is wired into `build.beforeBuildCommand`, so a plain `npm run tauri build`
// always has the interpreter — locally and in CI alike, one wiring point. The
// build runner needs NO Python toolchain (we fetch a prebuilt binary, never
// compile), so the "no Python in CI to build" rule still holds.
//
// SCOPE: Windows + Linux (x86_64). macOS bundling is deferred (no universal2
// build; unsigned venv dylibs need signing — see docs/MACOS-SIGNING.md); there
// the app downloads a private interpreter at runtime instead (python_fetch.rs),
// so on macOS this stays a no-op. The interpreter is a fetched runtime artifact,
// never committed (git-ignored and asserted-untracked by check-files-in-place).
//
// Integrity: the release tag, per-platform asset, and SHA-256 are pinned below
// (the digests GitHub publishes per asset). The download is verified against
// that hash before it is unpacked — a tampered or truncated download fails
// loudly, no trust-on-first-use.
//
// Pure Node built-ins (global fetch + node:crypto), ESM, no dependencies.
// Extraction on Windows uses the system bsdtar (System32\tar.exe, present on
// Windows 10 1803+ and the windows-latest runner) addressed by absolute path —
// not bare `tar` — so a GNU/MSYS tar on PATH (e.g. under Git Bash), which
// misreads a `C:\…` path as a remote host, can't shadow it. Linux uses the
// system tar (GNU tar is universal there).

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import {
  mkdtempSync,
  renameSync,
  rmSync,
  mkdirSync,
  existsSync,
  readdirSync,
  readFileSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";
// Node 24 decompresses zstd natively, which is what makes the licence-bearing `.tar.zst` builds
// usable without asking every machine and every runner to install a `zstd` binary.
import { zstdDecompressSync } from "node:zlib";

// ---- the pin -------------------------------------------------------------
// CPython 3.12 matches the `python-smoke` CI job (broad wheel availability for
// the three pinned sidecar deps). One release tag for every platform, and the
// runtime-download pin in src-tauri/src/python_fetch.rs (macOS) is kept in
// lockstep with it. To advance: pick a new python-build-standalone release,
// update the tag + every asset hash from its SHA256SUMS, and bump the changelog.
const PY_VERSION = "3.12.13";
const PBS_TAG = "20260610";

// Per-platform asset + hash + the interpreter path that proves a good unpack.
//
// These are the `-full.tar.zst` builds, NOT `install_only.tar.gz`. The reason is licensing: the
// runtime links OpenSSL, SQLite, libffi, liblzma, mpdecimal, bzip2, expat and zlib, and the
// install_only archive carries only CPython's own LICENSE.txt — so PM's Windows and Linux installers
// conveyed all of those with no copy of their licences, several of which (Apache-2.0, MIT, BSD)
// require exactly that. The full archive ships `python/licenses/` with a file per component, taken
// from the build we actually pin, so the attribution is read off the artifact rather than sourced
// from someone's documentation and hoped to still be true.
//
// It is not the more expensive choice: zstd compresses better than gzip, so Windows is 41.4 MB
// against 44 MB and Linux is 106.4 MB against 105.9 MB. Node 24 decompresses zstd natively
// (`node:zlib`), so this needs no new tooling either.
//
// Layout differs from install_only: the full archive is `python/{install,licenses,build}/` plus
// `python/PYTHON.json`, so the unpack below moves `install/` into place and keeps `licenses/`
// beside it, discarding `build/` (object files and headers — a few hundred MB of nothing PM ships).
//
// Hashes are the digests GitHub publishes for each asset; the Windows one was additionally verified
// against a downloaded copy byte for byte.
//
// Linux additionally PRUNES the tcl/tk/tkinter family after extraction. Two
// reasons: (1) required — linuxdeploy walks every ELF in the AppDir and fails
// on `_tkinter`'s `libtcl9.0.so` dependency (it resolves through the module's
// $ORIGIN rpath, which linuxdeploy doesn't follow), killing the AppImage
// bundle; (2) lean — the sidecar is headless (markitdown / fastembed /
// faster-whisper), nothing imports tkinter, and the family is ~25 MB of dead
// weight. `pruneRev` is folded into the stamp so editing the prune list forces
// a re-extract on machines holding an older tree.
const PLATFORMS = {
  win32: {
    asset: `cpython-${PY_VERSION}+${PBS_TAG}-x86_64-pc-windows-msvc-pgo-full.tar.zst`,
    sha256: "cba72a21ed4e59794eb5cf4672797204b19926feee79896bc097b7416ed75e8b",
    interpreter: ["python.exe"],
  },
  linux: {
    asset: `cpython-${PY_VERSION}+${PBS_TAG}-x86_64-unknown-linux-gnu-pgo+lto-full.tar.zst`,
    sha256: "15373dfc976a3bdd6e1855aa87f247bd71157416abf9a3091fd0acf9b50983b0",
    interpreter: ["bin", "python3"],
    pruneRev: " prune3",
    prune: {
      // Exact stdlib entries (rmSync force ignores any that don't exist).
      paths: [
        "lib/python3.12/lib-dynload/_tkinter.cpython-312-x86_64-linux-gnu.so",
        "lib/python3.12/tkinter",
        "lib/python3.12/idlelib",
        "lib/python3.12/turtledemo",
        "lib/python3.12/turtle.py",
      ],
      // Everything in lib/ belonging to the tcl/tk runtime (libtcl9.0.so,
      // libtcl9tk9.0.so, tcl9.0/, tk9.0/, itcl4.3.5/, thread3.0.4/, …).
      libPrefixes: ["libtcl", "libtk", "tcl", "tk", "itcl", "thread"],
    },
  },
};

const platform = PLATFORMS[process.platform];
if (!platform) {
  console.log(
    `fetch-python: no bundled interpreter for ${process.platform} — skipping ` +
      `(macOS downloads a private copy at runtime instead; dev builds fall back to system Python).`,
  );
  process.exit(0);
}
const { asset: ASSET, sha256: SHA256 } = platform;
const URL = `https://github.com/astral-sh/python-build-standalone/releases/download/${PBS_TAG}/${ASSET}`;

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const destParent = join(repoRoot, "src-tauri");
const destDir = join(destParent, "python");
const stampFile = join(destDir, ".pm-pyver");
// Stamp identity = version+tag+hash (+ prune revision where one applies), so any
// change to the pin or the prune list forces a re-fetch.
const STAMP = `${PY_VERSION}+${PBS_TAG} ${SHA256}${platform.pruneRev ?? ""}`;

const exe = join(destDir, ...platform.interpreter);
if (existsSync(exe) && existsSync(stampFile) && readFileSync(stampFile, "utf8").trim() === STAMP) {
  console.log(`fetch-python: ${ASSET} already present and verified — skipping.`);
  process.exit(0);
}

const tmp = mkdtempSync(join(tmpdir(), "pm-python-"));
try {
  console.log(`fetch-python: downloading ${ASSET} …`);
  const res = await fetch(URL); // fetch follows GitHub's redirect to the CDN.
  if (!res.ok) {
    throw new Error(`download failed: HTTP ${res.status} ${res.statusText} for ${URL}`);
  }
  const bytes = Buffer.from(await res.arrayBuffer());

  const got = createHash("sha256").update(bytes).digest("hex");
  if (got !== SHA256) {
    throw new Error(`SHA-256 mismatch for ${ASSET}:\n  expected ${SHA256}\n  got      ${got}`);
  }
  console.log(`fetch-python: verified SHA-256 (${(bytes.length / 1048576).toFixed(1)} MB).`);

  // zstd, decompressed with Node's own zlib (24+) rather than a `zstd` binary nobody has installed.
  // System `tar` handles the resulting .tar on both Windows (bsdtar) and Linux.
  const archive = join(tmp, "python.tar");
  writeFileSync(archive, zstdDecompressSync(bytes));

  // Replace any stale interpreter wholesale (e.g. after a pin bump), then unpack.
  rmSync(destDir, { recursive: true, force: true });
  mkdirSync(destParent, { recursive: true });
  const sysTar =
    process.platform === "win32"
      ? join(process.env.SystemRoot || "C:\\Windows", "System32", "tar.exe")
      : "/usr/bin/tar";
  const tarExe = existsSync(sysTar) ? sysTar : "tar";

  // Unpack beside the destination — same volume, so the moves below are renames rather than copies,
  // and a failure leaves a staging dir rather than a half-written `python/`.
  const staging = join(destParent, ".pm-python-staging");
  rmSync(staging, { recursive: true, force: true });
  mkdirSync(staging, { recursive: true });
  try {
    // `build/` is object files and headers — hundreds of MB PM never ships. Excluded at extraction
    // so it is never written to disk at all.
    execFileSync(tarExe, ["-xf", archive, "-C", staging, "--exclude", "python/build"], {
      stdio: "inherit",
    });

    const unpacked = join(staging, "python");
    const installed = join(unpacked, "install");
    const licences = join(unpacked, "licenses");
    if (!existsSync(installed)) {
      throw new Error(`unpacked archive but ${installed} is missing — unexpected archive layout`);
    }
    // The component licences are the whole reason for the full archive; a build that stopped
    // shipping them must fail loudly, not quietly produce an unattributed bundle.
    if (!existsSync(licences)) {
      throw new Error(
        `unpacked archive but ${licences} is missing — this build carries no component licences, ` +
          `which is the reason PM fetches the full archive rather than install_only`,
      );
    }
    renameSync(installed, destDir);
    renameSync(licences, join(destDir, "licenses"));
  } finally {
    rmSync(staging, { recursive: true, force: true });
  }

  if (!existsSync(exe)) {
    throw new Error(`unpacked archive but ${exe} is missing — unexpected archive layout`);
  }

  // Two things PM has no use for, dropped so the tree holds only what actually runs.
  //
  //   * **40 `.pdb` debug-symbol files — 82 MB.** These are NOT new: the `install_only` archive
  //     carried exactly the same 40 files, and nothing here ever pruned them, so every Windows
  //     installer PM has shipped has contained 82 MB of Python debug symbols. Found while measuring
  //     this change rather than by looking for it.
  //   * **CPython's own test suite (`Lib/test/`) — 31 MB**, which the full archive keeps and
  //     `install_only` strips. It carries the dummy certificates and private keys CPython tests TLS
  //     with, which `just gitleaks` correctly flags (18 findings, all fixtures). Shipping a stdlib
  //     test corpus inside PM was never wanted; the scanner objecting is a symptom, not the reason.
  //
  // Net effect, measured on Windows: the installed interpreter goes from 150 MB to 69 MB AND gains
  // the component licences. The licence-bearing archive is not a cost here — it is a saving.
  rmSync(join(destDir, "Lib", "test"), { recursive: true, force: true });
  rmSync(join(destDir, "lib", `python${PY_VERSION.split(".").slice(0, 2).join(".")}`, "test"), {
    recursive: true,
    force: true,
  });
  let symbols = 0;
  for (const rel of readdirSync(destDir, { recursive: true })) {
    if (typeof rel === "string" && rel.endsWith(".pdb")) {
      rmSync(join(destDir, rel), { force: true });
      symbols += 1;
    }
  }
  console.log(`fetch-python: dropped CPython's test suite and ${symbols} debug-symbol files.`);

  if (platform.prune) {
    for (const rel of platform.prune.paths) {
      rmSync(join(destDir, rel), { recursive: true, force: true });
    }
    const libDir = join(destDir, "lib");
    for (const name of readdirSync(libDir)) {
      if (platform.prune.libPrefixes.some((p) => name.startsWith(p))) {
        rmSync(join(libDir, name), { recursive: true, force: true });
      }
    }
    console.log(`fetch-python: pruned the tcl/tk/tkinter family from the bundle.`);
  }

  // Stamp LAST, after unpack + prune both succeeded, so an interrupted run can
  // never leave a stamped-but-wrong tree that the skip check above would trust.
  writeFileSync(stampFile, `${STAMP}\n`);
  console.log(`fetch-python: ready at ${destDir}`);
} finally {
  rmSync(tmp, { recursive: true, force: true });
}
