// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The sidecar's dependency locks are current, fully pinned and fully hashed.
//
// WHY A GATE AND NOT JUST A FILE. A lock only protects anything while it matches the thing it
// claims to lock. Edit sidecar/requirements.txt, forget `just lock-regen`, and pip installs the
// OLD resolution under `--require-hashes` — silently, because the lock is perfectly valid, just
// stale. So each lock stamps the SHA-256 of every input it was generated from, and this check
// recomputes them. That is also what lets the check stay ZERO-DEPENDENCY and offline
// (INVARIANTS.md I-18): it never resolves anything, it verifies that someone else did.
//
// It runs in check-fast (so pre-commit and every PR), in pr.yml's hygiene job, and in
// release.yml's guards job — a stale lock blocks a merge AND a release.
//
// What it proves:
//   * every lock is stamped for the CURRENT sidecar/requirements.txt, requirements-optional.txt
//     and (for the optional components) the current base lock;
//   * the resolution targets the MIN_PYTHON floor Rust actually enforces;
//   * every entry is pinned with `==` — no range can slip in;
//   * every entry carries at least one sha256, so `--require-hashes` has something to check;
//   * the optional locks cover exactly the pins whose source of truth is sidecar.rs.
//
// What it does NOT prove: that the resolution is what uv would produce today. That needs the
// network. `just lock-regen` + a clean `git status` is the check for that, and Dependabot is
// what surfaces the upgrades.

import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const SIDECAR_RS = "src-tauri/src/sidecar.rs";
const BASE_INPUT = "sidecar/requirements.txt";
const BASE_LOCK = "sidecar/requirements.lock";
const OPTIONAL_INPUT = "sidecar/requirements-optional.txt";

/** Fewest entries a lock may legitimately contain. A regex that quietly stops matching reports a
 *  clean scan of nothing; the floors turn that into a failure. Same guard as check-action-pins. */
const ENTRY_FLOOR = {
  [BASE_LOCK]: 40,
  "sidecar/requirements-ocr.lock": 5,
  "sidecar/requirements-tsne.lock": 5,
};

/** PEP 503 name normalisation — uv writes `opentsne`, Rust pins `openTSNE`. */
export function normalise(name) {
  return name.replace(/[-_.]+/g, "-").toLowerCase();
}

export function sha256(text) {
  return createHash("sha256").update(text, "utf8").digest("hex");
}

/** The `# pm-*` stamps the generator writes. Returns partial data plus its own problems. */
export function parseHeader(text) {
  const problems = [];
  const stamp = (key) => {
    const m = text.match(new RegExp(`^# pm-${key}:[ \\t]*(.+)$`, "m"));
    return m ? m[1].trim() : null;
  };
  const version = stamp("lock");
  if (version !== "1") {
    problems.push(
      `missing or unknown \`# pm-lock:\` stamp (found ${version ?? "nothing"}) — regenerate with \`just lock-regen\``,
    );
  }
  const source = (raw) => {
    if (!raw) return null;
    const m = raw.match(/^(\S+)@sha256:([0-9a-f]{64})$/);
    return m ? { path: m[1], sha: m[2] } : null;
  };
  const input = source(stamp("input"));
  if (!input) problems.push("missing or malformed `# pm-input:` stamp");
  const rawConstraint = stamp("constraint");
  const constraint = rawConstraint ? source(rawConstraint) : null;
  if (rawConstraint && !constraint) problems.push("malformed `# pm-constraint:` stamp");
  const rawPins = stamp("pins");
  return {
    floor: stamp("python-floor"),
    input,
    constraint,
    pins: rawPins ? rawPins.split(/\s+/).filter(Boolean) : null,
    problems,
  };
}

/** Every pinned entry in a lock, with the hashes attached to it. */
export function parseEntries(text) {
  const lines = text.replace(/\r\n/g, "\n").split("\n");
  const entries = [];
  let current = null;
  for (const [i, line] of lines.entries()) {
    const pin = line.match(
      /^([A-Za-z0-9._-]+)\s*(==|>=|<=|~=|!=|>|<)\s*([^\s;\\]+)\s*(?:;\s*(.*?))?\s*\\?$/,
    );
    if (pin) {
      current = {
        line: i + 1,
        name: pin[1],
        operator: pin[2],
        version: pin[3],
        marker: (pin[4] ?? "").replace(/\s*\\$/, "").trim(),
        hashes: [],
      };
      entries.push(current);
      continue;
    }
    const hash = line.match(/^\s+--hash=sha256:([0-9a-f]{64})\s*\\?$/);
    if (hash && current) current.hashes.push(hash[1]);
    // A `# via …` comment or a blank line ends the entry; anything else is left alone.
    if (/^\s*(#|$)/.test(line) === false && !hash && !pin) current = null;
  }
  return entries;
}

/** Top-level requirements from a requirements.txt-shaped file, extras stripped. */
export function parseRequirements(text) {
  const out = [];
  for (const line of text.replace(/\r\n/g, "\n").split("\n")) {
    const trimmed = line.trim();
    if (!trimmed || trimmed.startsWith("#")) continue;
    const m = trimmed.match(/^([A-Za-z0-9._-]+)(?:\[[^\]]*\])?\s*==\s*([^\s;]+)/);
    if (m) out.push({ name: m[1], version: m[2], raw: trimmed });
  }
  return out;
}

/** The MIN_PYTHON floor and the optional pins, read from Rust — the source of truth for both. */
export function readRustFacts(rustSource) {
  const floor = rustSource.match(/const MIN_PYTHON:\s*\(u32,\s*u32\)\s*=\s*\((\d+),\s*(\d+)\)/);
  const tsne = rustSource.match(/const OPTIONAL_TSNE_PIN:\s*&str\s*=\s*"([^"]+)"/);
  const ocr = rustSource.match(/const OPTIONAL_OCR_PINS:\s*&\[&str\]\s*=\s*&\[([^\]]+)\]/);
  if (!floor || !tsne || !ocr)
    throw new Error(`could not read MIN_PYTHON / the optional pins from ${SIDECAR_RS}`);
  return {
    floor: `${floor[1]}.${floor[2]}`,
    pins: { tsne: [tsne[1]], ocr: [...ocr[1].matchAll(/"([^"]+)"/g)].map((m) => m[1]) },
  };
}

/**
 * Everything checkable about one lock, as a list of human-readable problems.
 * Pure: takes the file contents it needs, so the tests never touch the real tree.
 */
export function problemsForLock({ path, text, floor, inputs, expectedPins }) {
  const problems = [];
  const header = parseHeader(text);
  problems.push(...header.problems);

  if (header.floor !== floor) {
    problems.push(
      `resolved for Python ${header.floor ?? "(unstamped)"} but MIN_PYTHON in ${SIDECAR_RS} is ${floor} — ` +
        `raising the floor without regenerating leaves the lock covering versions PM no longer accepts, ` +
        `and lowering it leaves interpreters with no locked resolution at all`,
    );
  }

  for (const key of ["input", "constraint"]) {
    const stamped = header[key];
    if (!stamped) continue;
    const actual = inputs[stamped.path];
    if (actual === undefined) {
      problems.push(
        `\`# pm-${key}:\` names ${stamped.path}, which this check does not know how to read`,
      );
    } else if (sha256(actual) !== stamped.sha) {
      problems.push(
        `${stamped.path} has changed since this lock was generated — run \`just lock-regen\` and commit the result ` +
          `(stamped ${stamped.sha.slice(0, 12)}…, actual ${sha256(actual).slice(0, 12)}…)`,
      );
    }
  }

  const entries = parseEntries(text);
  const floorFor = ENTRY_FLOOR[path];
  if (floorFor !== undefined && entries.length < floorFor) {
    problems.push(
      `only ${entries.length} entries parsed (expected at least ${floorFor}) — the lock is truncated, or this parser has stopped matching`,
    );
    return problems;
  }

  for (const entry of entries) {
    if (entry.operator !== "==") {
      problems.push(
        `line ${entry.line}: \`${entry.name}${entry.operator}${entry.version}\` is not pinned — a lock may only use \`==\``,
      );
    }
    if (entry.hashes.length === 0) {
      problems.push(
        `line ${entry.line}: \`${entry.name}==${entry.version}\` carries no --hash, so --require-hashes cannot verify it`,
      );
    }
  }

  // Every pin the lock claims to cover is present, at that exact version, in every fork of it.
  const byName = new Map();
  for (const entry of entries) {
    const key = normalise(entry.name);
    if (!byName.has(key)) byName.set(key, []);
    byName.get(key).push(entry);
  }
  for (const pin of expectedPins) {
    const found = byName.get(normalise(pin.name));
    if (!found) {
      problems.push(
        `\`${pin.raw}\` is required but does not appear in this lock — run \`just lock-regen\``,
      );
      continue;
    }
    for (const entry of found) {
      if (entry.version !== pin.version) {
        problems.push(
          `line ${entry.line}: \`${pin.name}\` is pinned to ${pin.version} but the lock resolves it to ${entry.version} — run \`just lock-regen\``,
        );
      }
    }
  }
  return problems;
}

export function scan(root) {
  const read = (rel) => readFileSync(join(root, rel), "utf8");
  const { floor, pins } = readRustFacts(read(SIDECAR_RS));
  const baseInput = read(BASE_INPUT);
  const optionalInput = read(OPTIONAL_INPUT);
  const baseLock = read(BASE_LOCK);
  const inputs = {
    [BASE_INPUT]: baseInput,
    [OPTIONAL_INPUT]: optionalInput,
    [BASE_LOCK]: baseLock,
  };

  const failures = [];
  const record = (path, problems) => {
    for (const p of problems) failures.push(`${path}: ${p}`);
  };

  record(
    BASE_LOCK,
    problemsForLock({
      path: BASE_LOCK,
      text: baseLock,
      floor,
      inputs,
      expectedPins: parseRequirements(baseInput),
    }),
  );

  const optionalPins = parseRequirements(optionalInput);
  let entryCount = parseEntries(baseLock).length;
  for (const [name, rustPins] of [
    ["ocr", pins.ocr],
    ["tsne", pins.tsne],
  ]) {
    const path = `sidecar/requirements-${name}.lock`;
    const text = read(path);
    const header = parseHeader(text);
    // The lock must claim exactly the pins Rust holds — not a superset, not a stale subset.
    const claimed = (header.pins ?? []).join(" ");
    if (claimed !== rustPins.join(" ")) {
      failures.push(
        `${path}: \`# pm-pins:\` says "${claimed}" but ${SIDECAR_RS} pins "${rustPins.join(" ")}" — run \`just lock-regen\``,
      );
    }
    // …and those pins must also be the ones the audit file lists, so pip-audit scans what installs.
    for (const pin of rustPins) {
      if (!optionalPins.some((p) => `${p.name}==${p.version}` === pin)) {
        failures.push(
          `${path}: \`${pin}\` is not listed in ${OPTIONAL_INPUT}, so \`just pip-audit\` never scans it`,
        );
      }
    }
    const expected = rustPins.map((raw) => {
      const [n, v] = raw.split("==");
      return { name: n, version: v, raw };
    });
    record(path, problemsForLock({ path, text, floor, inputs, expectedPins: expected }));
    entryCount += parseEntries(text).length;
  }
  return { failures, entryCount };
}

function main() {
  const root = join(dirname(fileURLToPath(import.meta.url)), "..");
  const { failures, entryCount } = scan(root);
  if (failures.length > 0) {
    console.error("✗ requirements-lock:\n");
    for (const f of failures) console.error(`  ${f}`);
    console.error(
      "\nThe sidecar installs these with `--require-hashes`. Regenerate with `just lock-regen`.",
    );
    process.exit(1);
  }
  console.log(
    `✓ requirements-lock: ${entryCount} pinned, hashed entries across 3 locks, all stamped for the current ` +
      `requirements and MIN_PYTHON floor`,
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}
