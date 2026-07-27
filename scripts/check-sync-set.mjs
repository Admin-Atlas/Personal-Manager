// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Sync-set gate. SYNC-SET.md classifies every unit of truth as truth / derived /
// device / mixed, so a future sync core has a declared owner per table instead of
// having to excavate one out of a dozen shipped features. A register only works if
// it cannot fall behind the schema, so this asserts the two agree in both directions:
//
//   1. Every table the schema creates has a row in the register.
//   2. Every row in the register names a table the schema still creates.
//   3. Every class is one of the four documented ones.
//
// The schema side is parsed from the `CREATE TABLE` statements in migrations.rs —
// the same text that runs — with comments stripped and the test module cut off, so a
// `CREATE TABLE` mentioned in prose or built by a test helper can't count as real.
//
// Pure Node built-ins, ESM, no dependencies.

import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const read = (rel) => readFileSync(join(repoRoot, rel), "utf8").replace(/\r\n/g, "\n");

const CLASSES = new Set(["truth", "derived", "device", "mixed"]);
const MIGRATIONS = "src-tauri/src/db/migrations.rs";
const REGISTER = "SYNC-SET.md";

// --- 1. the schema's tables ------------------------------------------------

let sql = read(MIGRATIONS);

// Cut the test module: its helpers build tables with `format!("CREATE TABLE {name} …")`
// against throwaway stores. Those are not the product's schema.
const testMod = sql.indexOf("#[cfg(test)]");
if (testMod > 0) sql = sql.slice(0, testMod);

// Strip Rust line comments and SQL line comments. Several migration comments discuss
// "the stored CREATE TABLE text", which would otherwise parse as a table named `text`.
sql = sql.replace(/^[ \t]*\/\/.*$/gm, "").replace(/--[^\n]*/g, "");

const schemaTables = new Set(
  [
    ...sql.matchAll(/CREATE\s+(?:VIRTUAL\s+)?TABLE\s+(?:IF\s+NOT\s+EXISTS\s+)?([a-zA-Z_][\w]*)/g),
  ].map((m) => m[1]),
);

if (schemaTables.size === 0) {
  console.error(
    `✗ sync-set: parsed zero tables out of ${MIGRATIONS} — the parser is broken, not the register`,
  );
  process.exit(1);
}

// --- 2. the register's rows ------------------------------------------------

const doc = read(REGISTER);

// Scope to the "## The register" section so the class-glossary and the non-database
// tables (whose first cells are prose or paths, not bare table names) can't be read
// as register rows.
const start = doc.indexOf("## The register");
if (start < 0) {
  console.error(`✗ sync-set: ${REGISTER} has no "## The register" section`);
  process.exit(1);
}
const rest = doc.slice(start + 1);
const end = rest.indexOf("\n## ");
const section = end < 0 ? rest : rest.slice(0, end);

const problems = [];
const registered = new Map();

for (const line of section.split("\n")) {
  if (!line.trimStart().startsWith("|")) continue;
  const cells = line
    .split("|")
    .slice(1, -1)
    .map((c) => c.trim());
  if (cells.length < 2) continue;
  const table = cells[0].match(/^`([a-zA-Z_][\w]*)`$/)?.[1];
  if (!table) continue; // the header row and its `---` separator
  const cls = cells[1].replace(/\*/g, "").trim();
  if (registered.has(table)) problems.push(`duplicate register row for \`${table}\``);
  if (!CLASSES.has(cls)) {
    problems.push(`\`${table}\` has class "${cls}" — must be one of: ${[...CLASSES].join(", ")}`);
  }
  registered.set(table, cls);
}

// --- 3. the two must agree -------------------------------------------------

for (const t of [...schemaTables].sort()) {
  if (!registered.has(t)) {
    problems.push(
      `table \`${t}\` is created by the schema but has no row in ${REGISTER}  — add one (see its "Checklist for a PR that adds a table")`,
    );
  }
}
for (const t of [...registered.keys()].sort()) {
  if (!schemaTables.has(t)) {
    problems.push(
      `${REGISTER} lists \`${t}\`, which the schema no longer creates  — remove the row or fix the name`,
    );
  }
}

if (problems.length) {
  console.error("✗ sync-set: the register and the schema disagree:\n");
  for (const p of problems) console.error(`  • ${p}`);
  console.error(
    "\n  A table with no declared owner of truth is a table a future sync silently strands.",
  );
  process.exit(1);
}

console.log(`✓ sync-set: all ${schemaTables.size} tables classified in ${REGISTER}`);
