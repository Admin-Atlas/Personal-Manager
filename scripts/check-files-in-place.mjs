// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Files-in-place lint. The repo is public and ships an encrypted local-data /
// sidecar-venv / downloaded-model design that must NEVER end up in the tree.
// `.gitignore` is not a security boundary (a file can be force-added, or a new
// path can dodge the patterns), so this asserts the *tracked set itself* is clean
// rather than trusting ignore rules.
//
// Three checks, all over `git ls-files` (the actual tracked set):
//   1. Nothing forbidden is tracked — user data, secrets, runtime artifacts,
//      downloaded models, the Python venv, build output, the git-ignored docs/.
//   2. The repo root holds only its known entries (no stray dump/scratch/secret
//      committed at top level).
//   3. Python source lives only under sidecar/ (the one place Python belongs).
//
// Pure Node built-ins, ESM, no dependencies.

import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const tracked = execFileSync("git", ["ls-files"], { encoding: "utf8", cwd: repoRoot })
  .split("\n")
  .map((s) => s.trim())
  .filter(Boolean);

const problems = [];

// 1. Forbidden tracked paths. Each pattern mirrors a .gitignore rule whose job is
// confidentiality or cleanliness — here we prove it actually held.
const FORBIDDEN = [
  {
    // The user's Markdown vault / data / runtime dirs must live outside the repo.
    // `except` spares the Rust source module src-tauri/src/vault/ — that is feature
    // CODE (the vault key model), not user data.
    re: /(^|\/)(data|runtime|vault)\//,
    except: /^src-tauri\/src\/vault\//,
    why: "local user-data directory (must live outside the repo)",
  },
  { re: /\.sqlite($|-)/, why: "SQLite store / WAL (the user's encrypted data)" },
  {
    re: /(^|\/)\.venv\//,
    why: "Python virtualenv (a runtime artifact, provisioned on the user's machine)",
  },
  {
    re: /^src-tauri\/python\//,
    why: "bundled standalone interpreter (fetched at build time by fetch-python.mjs, never committed)",
  },
  { re: /(^|\/)__pycache__\//, why: "Python bytecode cache" },
  { re: /\.pyc$/, why: "compiled Python" },
  { re: /(^|\/)node_modules\//, why: "installed npm packages" },
  { re: /(^|\/)target\//, why: "Rust build output" },
  { re: /^dist(-ssr)?\//, why: "frontend build output" },
  { re: /\.(onnx|ort)$/, why: "downloaded ML model weights (fetched at runtime, never committed)" },
  { re: /(^|\/)\.env(\.|$)/, why: "environment file (may hold secrets)" },
  { re: /^docs\//, why: "docs/ is intentionally git-ignored (local-only until the repo move)" },
  { re: /^PUBLISH\.md$/, why: "internal publish runbook (never published)" },
  { re: /^V1 overview\//, why: "internal handoff notes (never published)" },
  {
    re: /(^|\/)security-review-.*\.md$/,
    why: "local security-review report (may quote secret-adjacent code)",
  },
  { re: /(^|\/)settings\.local\.json$/, why: "per-developer Claude settings (stays local)" },
];
for (const f of tracked) {
  for (const { re, except, why } of FORBIDDEN) {
    if (re.test(f) && !(except && except.test(f))) {
      problems.push(`tracked but must not be: ${f}  — ${why}`);
    }
  }
}

// 2. Root cleanliness — tracked top-level entries must be a known set. Adding a
// new legitimate root file? Add it here in the same PR; that is the point.
const ALLOWED_ROOT = new Set([
  ".claude",
  ".design-sync", // /design-sync skill inputs (claude.ai/design); build output is git-ignored
  ".github",
  ".gitignore",
  ".gitleaks.toml",
  ".pre-commit-config.yaml",
  ".prettierignore",
  ".prettierrc.json",
  ".vscode",
  "AGENTS.md",
  "CLAUDE.md",
  "CONTRIBUTING.md",
  "LICENCE.txt",
  "README.md",
  "RELEASING.md",
  "design-system-docs",
  "eslint.config.js",
  "index.html",
  "justfile",
  "package-lock.json",
  "package.json",
  "ruff.toml",
  "SECURITY.md",
  "scripts",
  "sidecar",
  "src",
  "src-tauri",
  "tsconfig.json",
  "tsconfig.node.json",
  "vite.config.ts",
]);
const rootEntries = new Set(tracked.map((f) => f.split("/")[0]));
for (const entry of [...rootEntries].sort()) {
  if (!ALLOWED_ROOT.has(entry)) {
    problems.push(
      `stray at repo root: ${entry}  — add it to ALLOWED_ROOT if it belongs, otherwise move/remove it`,
    );
  }
}

// 3. Python only under sidecar/.
for (const f of tracked) {
  if (f.endsWith(".py") && !f.startsWith("sidecar/")) {
    problems.push(`Python file outside sidecar/: ${f}  — the sidecar is the only Python in PM`);
  }
}

if (problems.length) {
  console.error("✗ files-in-place: found tracked files that should not be here:\n");
  for (const p of problems) console.error(`  • ${p}`);
  console.error("");
  process.exit(1);
}

console.log(`✓ files-in-place: ${tracked.length} tracked files, all in their proper place`);
