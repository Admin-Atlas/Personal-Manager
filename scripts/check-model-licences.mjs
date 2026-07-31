// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Every model in the curated local-AI catalogue is under a licence someone read and accepted, and
// the catalogue's copy of it still matches the ledger it came from.
//
// WHY. `src-tauri/local_models.json` is compiled into the binary and lists the models PM offers to
// download. Seven of them are under bespoke publisher terms rather than an open-source licence —
// the Gemma Terms of Use, Meta's Llama Community Licenses, Alibaba's agreement for the largest
// Qwen 2.5 — and the catalogue carried nothing about any of it, so the UI had nothing to show a
// user before telling their machine to fetch the weights.
//
// This is the third of the licence gates, after the crates (cargo-deny) and the npm bundle, and it
// takes the same posture as the sidecar one: the machine gathers evidence, a HUMAN decides, and an
// offline gate checks the two committed files still agree.
//
// WHY A HUMAN DECIDES. Every catalogued repo is a third-party GGUF conversion — bartowski, unsloth,
// ggml-org — not the publisher's own. A conversion's `license:` tag is a copy that can be stale or
// missing, and `gated` is no proxy either: the conversions are all ungated even where
// `google/gemma-3-*` and `meta-llama/*` gate the original. The trap that settles it is
// `Qwen2.5-72B`: Qwen 2.5 is Apache-2.0 at 7B and 14B, but the 72B ships under Alibaba's own terms.
// Any per-family rule marks it open, and it isn't.
//
// OFFLINE AND ZERO-DEPENDENCY (INVARIANTS.md I-18). It reads two committed JSON files and one Rust
// file and reaches nothing. It deliberately does NOT import the generator, which would be the
// obvious way to reach the seed list: `scripts/generate-local-catalog.mjs` imports
// `@huggingface/gguf` at module load, and pr.yml's `hygiene` job has no `npm ci` — so that gate
// would pass on a dev box and die only in CI. The seed lives in the ledger for exactly this reason.

import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");

export const CATALOGUE_FILE = "src-tauri/local_models.json";
export const LEDGER_FILE = "src-tauri/model_licences.json";
const CATALOG_RS = "src-tauri/src/local_catalog.rs";

/** Fewest models the catalogue can plausibly hold; below this something has truncated it. */
const MODEL_FLOOR = 10;

/** The licence block a catalogue entry carries, in the order the generator writes it. */
const LICENCE_FIELDS = ["id", "name", "url", "open", "summary"];

/**
 * The schema version the Rust side actually parses, read from its parse-guard assertion.
 *
 * The generator's `SCHEMA_VERSION` and this assertion are the two ends of the same contract, and a
 * version nobody compares is decoration. Reading it from the Rust source rather than restating it
 * means this gate cannot drift from the thing it is checking.
 */
export function rustSchemaVersion(rustSource) {
  const m = rustSource.match(/assert_eq!\(\s*cat\.schema_version,\s*(\d+)/);
  if (!m) throw new Error(`could not read the pinned schema_version from ${CATALOG_RS}`);
  return Number(m[1]);
}

/** The generator's own hash: sha256 over the entries as serialised, nothing else. */
export function contentHash(entries) {
  return "sha256:" + createHash("sha256").update(JSON.stringify(entries)).digest("hex");
}

export function scan(root) {
  const read = (rel) => JSON.parse(readFileSync(join(root, rel), "utf8"));
  const catalogue = read(CATALOGUE_FILE);
  const ledger = read(LEDGER_FILE);
  const problems = [];

  const terms = ledger.terms ?? {};
  const models = ledger.models ?? {};
  const entries = catalogue.entries ?? [];

  // 1. The ledger's own consistency: every decided licence names a real row in `terms`, and no row
  //    is still awaiting a decision.
  for (const [repo, row] of Object.entries(models)) {
    if (!row.licence) {
      problems.push(
        `${LEDGER_FILE}: ${repo} has no reviewed licence — it is new, or its upstream metadata ` +
          `changed and the recorded answer was withdrawn; read its \`evidence\` and fill \`licence\` in`,
      );
      continue;
    }
    if (!terms[row.licence]) {
      problems.push(
        `${LEDGER_FILE}: ${repo} is recorded as \`${row.licence}\`, which is not a row in \`terms\``,
      );
    }
  }

  // 2. Every licence the UI can show is complete. An empty summary is an empty dialog, and the
  //    dialog is the whole point of carrying `open: false`.
  for (const [id, term] of Object.entries(terms)) {
    if (!term.name?.trim()) problems.push(`${LEDGER_FILE}: terms.${id} has no name`);
    if (!term.url?.startsWith("https://")) {
      problems.push(`${LEDGER_FILE}: terms.${id} needs an https url (found ${term.url ?? "none"})`);
    }
    if (typeof term.open !== "boolean") {
      problems.push(`${LEDGER_FILE}: terms.${id} must say whether it is \`open\` (true/false)`);
    }
    if (!term.summary?.trim()) {
      problems.push(
        `${LEDGER_FILE}: terms.${id} has no summary — that text is what a user is shown before a ` +
          `restricted download, so it cannot be blank`,
      );
    }
    if (!Object.values(models).some((row) => row.licence === id)) {
      problems.push(
        `${LEDGER_FILE}: terms.${id} is not used by any model — a licence table nobody references ` +
          `rots into fiction; drop it or point a model at it`,
      );
    }
  }

  // 3. The catalogue agrees with the ledger, field by field. A hand-edit to either file, or a
  //    ledger correction that was never regenerated, shows up here rather than in the app.
  for (const entry of entries) {
    const row = models[entry.repo];
    if (!row) {
      problems.push(
        `${CATALOGUE_FILE}: ${entry.repo} is catalogued but has no row in ${LEDGER_FILE} — the ` +
          `ledger IS the generator's seed, so the only way here is a hand-edited catalogue; ` +
          `re-run \`just generate-local-catalog\``,
      );
      continue;
    }
    const expected = terms[row.licence];
    if (!expected) continue; // already reported above
    if (!entry.licence) {
      problems.push(`${CATALOGUE_FILE}: ${entry.repo} carries no licence block`);
      continue;
    }
    if (entry.licence.id !== row.licence) {
      problems.push(
        `${CATALOGUE_FILE}: ${entry.repo} says \`${entry.licence.id}\` but ${LEDGER_FILE} records ` +
          `\`${row.licence}\` — regenerate the catalogue`,
      );
      continue;
    }
    for (const field of LICENCE_FIELDS) {
      if (field === "id") continue;
      if (entry.licence[field] !== expected[field]) {
        problems.push(
          `${CATALOGUE_FILE}: ${entry.repo}'s licence \`${field}\` has drifted from ` +
            `${LEDGER_FILE}'s \`${row.licence}\` — regenerate the catalogue`,
        );
      }
    }
    const extra = Object.keys(entry.licence).filter((k) => !LICENCE_FIELDS.includes(k));
    if (extra.length > 0) {
      problems.push(
        `${CATALOGUE_FILE}: ${entry.repo}'s licence carries unknown field(s) ${extra.join(", ")} — ` +
          `the Rust struct is deny_unknown_fields, so this would panic at first use`,
      );
    }
  }

  // 4. The catalogue has not been hand-edited. Nothing else in the repo checks `content_hash` —
  //    it is `#[allow(dead_code)]` on the Rust side — so this is the only place a hand-edit shows.
  const hash = contentHash(entries);
  if (catalogue.content_hash !== hash) {
    problems.push(
      `${CATALOGUE_FILE}: content_hash is ${catalogue.content_hash} but the entries hash to ` +
        `${hash} — the file was edited by hand; re-run \`just generate-local-catalog\``,
    );
  }

  // 5. The schema version the generator wrote is the one Rust parses.
  const pinned = rustSchemaVersion(readFileSync(join(root, CATALOG_RS), "utf8"));
  if (catalogue.schema_version !== pinned) {
    problems.push(
      `${CATALOGUE_FILE}: schema_version ${catalogue.schema_version} but ${CATALOG_RS} pins ` +
        `${pinned} — one side was bumped without the other`,
    );
  }

  if (entries.length < MODEL_FLOOR) {
    problems.push(
      `only ${entries.length} catalogue entries (expected at least ${MODEL_FLOOR}) — the catalogue ` +
        `is truncated, or this parser has stopped matching`,
    );
  }
  const restricted = entries.filter((e) => e.licence && !e.licence.open).length;
  return { problems, count: entries.length, restricted, ledgerCount: Object.keys(models).length };
}

function main() {
  const { problems, count, restricted, ledgerCount } = scan(repoRoot);
  if (problems.length > 0) {
    console.error("✗ model-licences:\n");
    for (const p of problems) console.error(`  • ${p}`);
    process.exit(1);
  }
  console.log(
    `✓ model-licences: all ${count} catalogued models (${restricted} under restricted terms) carry ` +
      `a reviewed licence matching ${LEDGER_FILE}'s ${ledgerCount} rows`,
  );
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main();
}
