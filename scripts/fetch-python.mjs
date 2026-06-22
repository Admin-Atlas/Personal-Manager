// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Fetch the bundled, relocatable CPython for the packaged app (spec §4.6).
//
// PM's document sidecar needs Python. Rather than require the user to install
// it, the Windows build SHIPS a standalone interpreter (from python-build-
// standalone — the same relocatable builds `uv` uses) as a Tauri resource, and
// provisions the managed venv against it at first ingest. This script downloads
// that interpreter at BUILD time and unpacks it to `src-tauri/python/`, which
// `tauri.windows.conf.json` then bundles into the NSIS installer.
//
// It is wired into `build.beforeBuildCommand`, so a plain `npm run tauri build`
// always has the interpreter — locally and in CI alike, one wiring point. The
// build runner needs NO Python toolchain (we fetch a prebuilt binary, never
// compile), so the "no Python in CI to build" rule still holds.
//
// SCOPE: Windows only. macOS bundling is deferred (no universal2 build; unsigned
// venv dylibs need signing — see docs/MACOS-SIGNING.md). On any non-Windows host
// this is a no-op so the macOS/dev build proceeds exactly as before (system
// Python fallback). The interpreter is a fetched runtime artifact, never
// committed (it is git-ignored and asserted-untracked by check-files-in-place).
//
// Integrity: the release tag, asset, and SHA-256 are pinned below (taken from the
// release's signed SHA256SUMS). The download is verified against that hash before
// it is unpacked — a tampered or truncated download fails loudly, no
// trust-on-first-use.
//
// Pure Node built-ins (global fetch + node:crypto), ESM, no dependencies.
// Extraction uses the Windows system bsdtar (System32\tar.exe, present on
// Windows 10 1803+ and the windows-latest runner) addressed by absolute path —
// not bare `tar` — so a GNU/MSYS tar on PATH (e.g. under Git Bash), which
// misreads a `C:\…` path as a remote host, can't shadow it.

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { mkdtempSync, rmSync, mkdirSync, existsSync, readFileSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

// ---- the pin -------------------------------------------------------------
// CPython 3.12 matches the `python-smoke` CI job (broad wheel availability for
// the three pinned sidecar deps). To advance: pick a new python-build-standalone
// release, update all four fields from its SHA256SUMS, and bump the changelog.
const PY_VERSION = "3.12.13";
const PBS_TAG = "20260610";
const ASSET = `cpython-${PY_VERSION}+${PBS_TAG}-x86_64-pc-windows-msvc-install_only.tar.gz`;
const SHA256 = "f5e4d9f856567493776f3d1e832c939fbaba5dcbcc5e0492a82ecfceea83b316";
const URL = `https://github.com/astral-sh/python-build-standalone/releases/download/${PBS_TAG}/${ASSET}`;

// `install_only` archives unpack to a top-level `python/` dir holding
// `python.exe` (plus Lib/, DLLs/, a bundled pip/venv, and LICENSE.txt).
const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const destParent = join(repoRoot, "src-tauri");
const destDir = join(destParent, "python");
const stampFile = join(destDir, ".pm-pyver");
// Stamp identity = version+tag+hash, so any change to the pin forces a re-fetch.
const STAMP = `${PY_VERSION}+${PBS_TAG} ${SHA256}`;

if (process.platform !== "win32") {
  console.log(
    `fetch-python: bundled interpreter is Windows-only for now; skipping on ${process.platform} ` +
      `(the build falls back to system Python, unchanged).`,
  );
  process.exit(0);
}

const exe = join(destDir, "python.exe");
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

  const archive = join(tmp, ASSET);
  writeFileSync(archive, bytes);

  // Replace any stale interpreter wholesale (e.g. after a pin bump), then unpack.
  rmSync(destDir, { recursive: true, force: true });
  mkdirSync(destParent, { recursive: true });
  const sysTar = join(process.env.SystemRoot || "C:\\Windows", "System32", "tar.exe");
  const tarExe = existsSync(sysTar) ? sysTar : "tar";
  execFileSync(tarExe, ["-xzf", archive, "-C", destParent], { stdio: "inherit" });

  if (!existsSync(exe)) {
    throw new Error(`unpacked archive but ${exe} is missing — unexpected archive layout`);
  }
  writeFileSync(stampFile, `${STAMP}\n`);
  console.log(`fetch-python: ready at ${destDir}`);
} finally {
  rmSync(tmp, { recursive: true, force: true });
}
