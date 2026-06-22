// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// AGPL hygiene: every first-party source file carries the two-line SPDX header,
// and the verbatim AGPL licence text is untouched.
//
//   1. HEADERS — each tracked .rs/.ts/.tsx/.js/.mjs/.py/.css file begins with
//        SPDX-FileCopyrightText: <year> <author>
//        SPDX-License-Identifier: AGPL-3.0-or-later
//      (generated/third-party reference under design-system-docs/ is excluded —
//      e.g. support.js is a "do not edit" bundle and must not be re-licensed).
//
//   2. LICENCE.txt UNCHANGED — pinned by SHA-256. The file is the verbatim FSF
//      AGPL-3.0 text; it must never be edited, only replaced wholesale by a future
//      FSF revision (which would be a deliberate, reviewed change to this hash).
//
// Pure Node built-ins, ESM, no dependencies.

import { readFileSync } from "node:fs";
import { createHash } from "node:crypto";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const read = (rel) => readFileSync(join(repoRoot, rel), "utf8");

// The verbatim AGPL-3.0 text (LICENCE.txt). Editing the licence — even a stray
// character — changes this; replacing it for a new FSF revision means updating
// this constant in the same, reviewed commit.
const LICENCE_FILE = "LICENCE.txt";
const LICENCE_SHA256 = "e0eedba615d5cd1b986afb6c5b3a4b1ae33713e7e9dc74d19daec5e3221f9d2e";

const HEADER_EXTENSIONS = ["rs", "ts", "tsx", "js", "mjs", "py", "css"];
// Generated bundles / third-party design reference — not first-party source.
const HEADER_EXCLUDE = [/^design-system-docs\//];

const problems = [];

// 1. Headers.
const tracked = execFileSync("git", ["ls-files", ...HEADER_EXTENSIONS.map((e) => `*.${e}`)], {
  encoding: "utf8",
  cwd: repoRoot,
})
  .split("\n")
  .map((s) => s.trim())
  .filter(Boolean)
  .filter((f) => !HEADER_EXCLUDE.some((re) => re.test(f)));

let checked = 0;
for (const f of tracked) {
  const head = read(f).split(/\r?\n/).slice(0, 5).join("\n");
  const hasCopyright = /SPDX-FileCopyrightText:/.test(head);
  const hasLicence = /SPDX-License-Identifier:\s*AGPL-3\.0-or-later/.test(head);
  if (!hasCopyright || !hasLicence) {
    const missing = [
      !hasCopyright && "SPDX-FileCopyrightText",
      !hasLicence && "SPDX-License-Identifier: AGPL-3.0-or-later",
    ]
      .filter(Boolean)
      .join(" + ");
    problems.push(`missing header (${missing}): ${f}`);
  }
  checked++;
}

// 2. Licence integrity.
const sha = createHash("sha256")
  .update(readFileSync(join(repoRoot, LICENCE_FILE)))
  .digest("hex");
if (sha !== LICENCE_SHA256) {
  problems.push(
    `${LICENCE_FILE} has changed — sha256 ${sha} ≠ expected ${LICENCE_SHA256}. ` +
      `The AGPL text must stay verbatim; if this is an intentional FSF-revision replacement, update LICENCE_SHA256.`,
  );
}

if (problems.length) {
  console.error("✗ spdx/licence:\n");
  for (const p of problems) console.error(`  • ${p}`);
  console.error("\nAdd the two-line header (see an existing source file) to each flagged file.");
  process.exit(1);
}

console.log(`✓ spdx/licence: ${checked} source files carry the header; ${LICENCE_FILE} unchanged`);
