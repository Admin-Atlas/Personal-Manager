// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Release safety check (audit T-03): prove every updater signature (.sig) was made
// by the key whose PUBLIC half ships in tauri.conf.json — the key every desktop
// pins. A build signed with a rotated / wrong / corrupted key still produces
// perfectly well-formed .sig files, so verify-updater-manifest.mjs (which only
// checks PRESENCE) passes and the release publishes green — after which every
// desktop's updater silently REJECTS the update. That is a fleet-wide, silent
// brick, and nothing in the pipeline catches it today.
//
// This closes the gap by matching the 8-byte minisign key-id embedded in each
// signature against the key-id in the shipped pubkey. It is deterministic,
// dependency-free and cross-platform — no minisign binary, no ed25519/prehash-mode
// assumptions.
//
// Scope note: this asserts key IDENTITY (the signature was made by the shipped
// key) — exactly the "wrong/rotated key bricks the fleet" failure the audit flags.
// It does not re-run full ed25519 byte verification; that needs minisign's exact
// prehash mode, which we can't pin without a live signing key to test against. The
// workflow_dispatch signing dry-run (.github/workflows/signing-dryrun.yml) exercises
// the real key end-to-end and is where a stronger check would live.
//
// Env:
//   SIG_DIR     directory scanned (recursively) for *.sig updater signatures
//   TAURI_CONF  path to tauri.conf.json holding plugins.updater.pubkey
//               (default: src-tauri/tauri.conf.json)
//
// Pure Node built-ins, ESM, no dependencies.

import { readdirSync, statSync, readFileSync, existsSync } from "node:fs";
import { join } from "node:path";

const { SIG_DIR, TAURI_CONF = "src-tauri/tauri.conf.json" } = process.env;
if (!SIG_DIR) throw new Error("Missing required env var: SIG_DIR");
if (!existsSync(SIG_DIR)) throw new Error(`SIG_DIR does not exist: ${SIG_DIR}`);

// Tauri stores the pubkey (in tauri.conf.json) and each .sig file as base64 of the
// minisign 2-line text. A raw minisign file starts with "untrusted comment:"; if
// the input already does it wasn't base64-wrapped — handle both so the check is
// robust to either convention.
function minisignText(raw) {
  const s = raw.trim();
  if (s.startsWith("untrusted comment:")) return s;
  return Buffer.from(s, "base64").toString("utf8");
}

// The base64 payload line (line 2) of a minisign public key or signature decodes to
// [2-byte algorithm][8-byte key id][key|signature]. Both pubkey and signature carry
// the key id at the same offset, which is all we compare.
function keyIdOf(raw, what) {
  const lines = minisignText(raw)
    .split("\n")
    .map((l) => l.trim())
    .filter(Boolean);
  if (lines.length < 2) throw new Error(`${what}: not a minisign key/signature (need ≥2 lines)`);
  const payload = Buffer.from(lines[1], "base64");
  if (payload.length < 10) {
    throw new Error(`${what}: minisign payload too short (${payload.length} bytes)`);
  }
  return payload.subarray(2, 10);
}

// minisign displays the key id as its 8 bytes reversed, uppercase hex.
const idHex = (id) => Buffer.from(id).reverse().toString("hex").toUpperCase();

const conf = JSON.parse(readFileSync(TAURI_CONF, "utf8"));
const pubkey = conf?.plugins?.updater?.pubkey;
if (!pubkey) throw new Error(`${TAURI_CONF}: plugins.updater.pubkey is missing`);
const wantId = keyIdOf(pubkey, "tauri.conf.json pubkey");

function walk(dir) {
  const out = [];
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) out.push(...walk(p));
    else if (p.endsWith(".sig")) out.push(p);
  }
  return out;
}

const sigs = walk(SIG_DIR);
if (sigs.length === 0) throw new Error(`No .sig files found under ${SIG_DIR}`);

const problems = [];
for (const sig of sigs) {
  const gotId = keyIdOf(readFileSync(sig, "utf8"), sig);
  if (!wantId.equals(gotId)) {
    problems.push(`${sig}: signed by key ${idHex(gotId)}, not the shipped key ${idHex(wantId)}`);
  }
}

if (problems.length) {
  console.error(`✗ updater signature key mismatch (shipped key ${idHex(wantId)}):\n`);
  for (const p of problems) console.error(`  • ${p}`);
  console.error(
    "\n  A build signed with a key the desktops don't pin would brick fleet auto-update.",
  );
  process.exit(1);
}

console.log(`✓ ${sigs.length} updater signature(s) all made by the shipped key ${idHex(wantId)}`);
