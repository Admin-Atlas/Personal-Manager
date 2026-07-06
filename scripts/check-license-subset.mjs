// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Licence-list subset gate (audit T-02). Two configs name the accepted licence set
// independently: src-tauri/deny.toml drives the PR gate (cargo-deny proves the tree
// only uses these), and src-tauri/about.toml drives the third-party NOTICE generated
// at release (cargo-about). They are kept in step by a comment only — so a PR that
// adds a licence to deny.toml and forgets about.toml is green, and the failure fires
// in the PUBLISH job, AFTER build and signing (the release-only trap this repo
// remembers). This asserts the relation the comment claims:
//
//     deny.allow ∪ deny.[[licenses.exceptions]].allow  ⊆  about.accepted
//
// i.e. every licence the tree is allowed to use is attributable in the NOTICE. The
// reverse (about listing an extra licence) is harmless and not checked. The lists are
// flat quoted-string arrays, so a scoped regex extraction is enough — no TOML parser,
// no dependencies.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const read = (rel) => readFileSync(join(repoRoot, rel), "utf8").replace(/\r\n/g, "\n");

// Strip `#` comments so a licence named only inside a comment can't be miscounted.
const stripComments = (s) => s.replace(/(^|\s)#[^\n]*/g, "");

// Every "..."-quoted string inside a region.
const quoted = (s) => [...stripComments(s).matchAll(/"([^"]+)"/g)].map((m) => m[1]);

// The captured body of a `key = [ ... ]` array (first match).
function arrayBody(text, key) {
  const m = text.match(new RegExp(`${key}\\s*=\\s*\\[([\\s\\S]*?)\\]`));
  if (!m) throw new Error(`could not find a '${key} = [ … ]' array`);
  return m[1];
}

// --- deny.toml: the [licenses] allow list + every [[licenses.exceptions]] allow ---
const deny = read("src-tauri/deny.toml");
// The licences region runs from `[licenses]` to the next top-level table ([bans] /
// [sources]); `[[licenses.exceptions]]` is a sub-table and stays inside it. Scoping
// here keeps a future `[bans].allow` (a crate list) from being read as a licence.
const licStart = deny.search(/^\[licenses\]/m);
if (licStart === -1) throw new Error("deny.toml: no [licenses] table");
const afterLic = deny.slice(licStart + 1);
const nextTable = afterLic.search(/^\[[^[]/m); // single-bracket header, not [[...]]
const licRegion =
  nextTable === -1 ? deny.slice(licStart) : deny.slice(licStart, licStart + 1 + nextTable);

const denyLicences = new Set();
for (const m of licRegion.matchAll(/allow\s*=\s*\[([\s\S]*?)\]/g)) {
  for (const lic of quoted(m[1])) denyLicences.add(lic);
}
if (denyLicences.size === 0)
  throw new Error("deny.toml: found no allowed licences — parse likely broke");

// --- about.toml: the accepted list ----------------------------------------
const accepted = new Set(quoted(arrayBody(read("src-tauri/about.toml"), "accepted")));
if (accepted.size === 0)
  throw new Error("about.toml: found no accepted licences — parse likely broke");

// --- the subset assertion -------------------------------------------------
const missing = [...denyLicences].filter((lic) => !accepted.has(lic)).sort();
if (missing.length) {
  console.error("✗ licence subset: deny.toml allows licences that about.toml does not accept:\n");
  for (const lic of missing) console.error(`  • ${lic}`);
  console.error(
    "\n  Add them to src-tauri/about.toml's `accepted` list, or the NOTICE generation" +
      "\n  will fail in the release publish job (after build + signing).",
  );
  process.exit(1);
}

console.log(
  `✓ licence subset: all ${denyLicences.size} deny.toml licences are in about.toml's ${accepted.size} accepted`,
);
