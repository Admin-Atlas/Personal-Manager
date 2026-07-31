// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Regenerates the sidecar's hash-pinned dependency locks. NOT part of `just check` — it
// reaches the network and rewrites generated files. Run it (`just lock-regen`) whenever
// sidecar/requirements.txt or the optional pins in sidecar.rs change, then commit the result;
// `just check-requirements-lock` is what fails if you forget.
//
// Needs `uv` on PATH. uv rather than pip-tools for one reason that decided the whole shape of
// this: `--universal` resolves across every platform AND every Python version at once, emitting
// environment markers where the resolution forks. One file therefore covers Windows, macOS and
// Linux and every CPython from MIN_PYTHON upwards — which matters because MIN_PYTHON is 3.10 and
// macOS prefers a system interpreter over PM's downloaded 3.12, so the real deployed range is
// much wider than "the version we ship".
//
// Run from the repo root: uv writes `# via -r sidecar/requirements.txt` annotations using the
// path it was given, so a relative path is what keeps the committed file machine-independent.
// The optional components are fed on STDIN instead — their pins live in Rust, not in a file uv
// could name, and stdin leaves no path in the output at all.

import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
const SIDECAR_RS = "src-tauri/src/sidecar.rs";
const BASE_INPUT = "sidecar/requirements.txt";
const BASE_LOCK = "sidecar/requirements.lock";

const read = (rel) => readFileSync(join(ROOT, rel), "utf8");

// CRLF is normalised away before hashing, and check-requirements-lock.mjs does the same.
// `.gitattributes` pins the repo to `eol=lf`, but a Windows working copy can hold CRLF — hashing
// raw bytes stamps a digest only this machine can reproduce, and the gate then fails on every Linux
// runner while passing here.
const sha256 = (text) =>
  createHash("sha256").update(text.replace(/\r\n/g, "\n"), "utf8").digest("hex");

/** The `MIN_PYTHON` floor, read from Rust so the lock can never target a version PM won't use. */
export function pythonFloor(rustSource) {
  const m = rustSource.match(/const MIN_PYTHON:\s*\(u32,\s*u32\)\s*=\s*\((\d+),\s*(\d+)\)/);
  if (!m) throw new Error(`could not find MIN_PYTHON in ${SIDECAR_RS}`);
  return `${m[1]}.${m[2]}`;
}

/** The optional components' pins, read from Rust — the source of truth for both (L-6). */
export function optionalPins(rustSource) {
  const tsne = rustSource.match(/const OPTIONAL_TSNE_PIN:\s*&str\s*=\s*"([^"]+)"/);
  const ocr = rustSource.match(/const OPTIONAL_OCR_PINS:\s*&\[&str\]\s*=\s*&\[([^\]]+)\]/);
  if (!tsne || !ocr) throw new Error(`could not find the optional pins in ${SIDECAR_RS}`);
  return {
    tsne: [tsne[1]],
    ocr: [...ocr[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]),
  };
}

function header(lines) {
  return [
    "# PM sidecar dependency lock - GENERATED, DO NOT EDIT BY HAND.",
    "#",
    "# Regenerate with `just lock-regen` (needs `uv` on PATH) and commit the result.",
    "# `just requirements-lock` fails if this file drifts from the inputs stamped below. It runs in",
    "# check-fast, in pr.yml's hygiene job and in release.yml's guards job, so a stale lock blocks a",
    "# merge AND a release.",
    "#",
    "# Every entry is pinned with `==` and carries the SHA-256 of every artifact PyPI publishes for",
    "# that version, and the sidecar installs with `--require-hashes`: an artifact whose digest does",
    "# not match is refused rather than executed. Before this file existed only the top-level packages",
    "# were pinned, and every transitive dependency was whatever PyPI served on the day a user first",
    "# ran PM - in the one process that opens untrusted PDFs, documents and images.",
    "#",
    "# Universal resolution: ONE file covers Windows, macOS and Linux and every CPython from the",
    "# MIN_PYTHON floor upwards, with environment markers where the resolution forks by interpreter",
    "# (numpy, onnxruntime, pandas, av) or by platform (colorama, magika, pyreadline3, hf-xet).",
    "#",
    ...lines,
    "",
  ].join("\n");
}

function compile({ label, args, stdin, stamps }) {
  process.stderr.write(`lock-regen: resolving ${label} ...\n`);
  const body = execFileSync("uv", args, {
    cwd: ROOT,
    input: stdin,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
  });
  // uv emits CRLF on Windows; `.gitattributes` pins the tree to LF, so normalise here rather
  // than leaving a file that reports as modified the moment it is checked out elsewhere.
  const normalised = body.replace(/\r\n/g, "\n").replace(/^\n+/, "");
  return header(stamps) + normalised;
}

const UV = ["pip", "compile", "--universal", "--generate-hashes", "--no-header"];

function main() {
  const rust = read(SIDECAR_RS);
  const floor = pythonFloor(rust);
  const pins = optionalPins(rust);
  const baseInputHash = sha256(read(BASE_INPUT));

  const base = compile({
    label: BASE_LOCK,
    args: [...UV, BASE_INPUT, "--python-version", floor],
    stamps: [
      "# pm-lock: 1",
      `# pm-python-floor: ${floor}`,
      `# pm-input: ${BASE_INPUT}@sha256:${baseInputHash}`,
    ],
  });
  writeFileSync(join(ROOT, BASE_LOCK), base);

  // The optional locks resolve AGAINST the base lock. Without that constraint an on-demand
  // install could move a package the base venv already depends on — pip would happily swap the
  // numpy that fastembed and onnxruntime are running on, mid-session, to satisfy rapidocr.
  const baseLockHash = sha256(base);
  const optionalInputHash = sha256(read("sidecar/requirements-optional.txt"));
  for (const [name, list] of [
    ["ocr", pins.ocr],
    ["tsne", pins.tsne],
  ]) {
    const out = `sidecar/requirements-${name}.lock`;
    const body = compile({
      label: out,
      args: [...UV, "-", "--python-version", floor, "--constraint", BASE_LOCK],
      stdin: list.join("\n") + "\n",
      stamps: [
        "# pm-lock: 1",
        `# pm-python-floor: ${floor}`,
        `# pm-input: sidecar/requirements-optional.txt@sha256:${optionalInputHash}`,
        `# pm-constraint: ${BASE_LOCK}@sha256:${baseLockHash}`,
        `# pm-pins: ${list.join(" ")}`,
      ],
    });
    writeFileSync(join(ROOT, out), body);
  }

  process.stderr.write("lock-regen: done. Run `just requirements-lock` to verify.\n");
}

// Run only when invoked directly, so the test can import the pure helpers without shelling out.
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}
