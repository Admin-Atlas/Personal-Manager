// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// ESLint flat config. A lean baseline FLOOR: JS + TypeScript recommended
// (syntactic, not type-checked — type errors are already caught by `tsc --noEmit`)
// plus the classic React Hooks rules that catch real bugs. Formatting is owned by
// Prettier, so eslint-config-prettier turns off any rules that would fight it.
//
// Note: eslint-plugin-react-hooks v7's `recommended` preset also turns on the new
// React-Compiler rules (refs / immutability / set-state-in-effect). Those flag
// idiomatic patterns throughout the existing, shipped code, so they are NOT part
// of this floor — enable them as a deliberate, code-changing pass later.

import js from "@eslint/js";
import globals from "globals";
import tseslint from "typescript-eslint";
import reactHooks from "eslint-plugin-react-hooks";
import reactRefresh from "eslint-plugin-react-refresh";
import prettier from "eslint-config-prettier";

export default tseslint.config(
  // src-tauri/python is the fetched standalone interpreter (a build artifact, like
  // src-tauri/target) — present only on Windows checkouts and full of vendored JS.
  {
    ignores: [
      "dist",
      "dist-ssr",
      "src-tauri/target",
      "src-tauri/python",
      "design-system-docs",
      // The spec + decision log + scratch notes (git-ignored, local-only — not first-party
      // source). Lint shouldn't reach into it (e.g. a stray docs/calendar-view/*.js prototype).
      "docs",
      // Agent worktrees are whole checkouts of THIS repo on other branches (git-ignored,
      // local-only). Linting them audits code that isn't on the branch under test: the gate
      // double-counts every finding, and a half-finished experiment in a worktree could fail
      // `just check` on a clean main, pointing at a path that isn't part of the build.
      ".claude/worktrees",
      "node_modules",
      // design-sync (claude.ai/design) build output & staged converter scripts —
      // regenerated, vendored, git-ignored. The hand-authored .design-sync/previews/
      // and preview-provider.tsx are NOT ignored (they're first-party and linted).
      "ds-bundle",
      ".ds-sync",
      ".design-sync/.cache",
    ],
  },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  // Frontend (browser). Type-aware for the one bug class tsc can't catch: dropped
  // promises. Every backend call is an async `invoke` through src/lib/ipc.ts, so a
  // forgotten await/.catch silently swallows an IPC error (and under StrictMode
  // doubles a fire-and-forget effect). `projectService` turns on type info just for
  // src/**; the rest of the type-aware set stays off — this is the promise floor,
  // not full recommendedTypeChecked (T-08).
  {
    files: ["src/**/*.{ts,tsx}"],
    languageOptions: {
      ecmaVersion: 2022,
      globals: globals.browser,
      parserOptions: {
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
      },
    },
    plugins: {
      "react-hooks": reactHooks,
      "react-refresh": reactRefresh,
    },
    rules: {
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "warn",
      "react-refresh/only-export-components": ["warn", { allowConstantExport: true }],
      "@typescript-eslint/no-floating-promises": "error",
      "@typescript-eslint/no-misused-promises": [
        "error",
        { checksVoidReturn: { attributes: false } },
      ],
      // The backend boundary (AGENTS.md "src/lib/ipc.ts — the only place that calls Rust").
      // Ban `invoke`/`Channel` (the `@tauri-apps/api/core` entrypoint) everywhere so every
      // backend command goes through the typed wrappers in ipc.ts, which normalise errors and
      // keep the surface auditable. Scoped to the exact path, NOT `@tauri-apps/api/*` — the
      // builtin plugin APIs (api/event, api/app, api/window, api/webview, plugin-*) legitimately
      // cross from ~15 components and must not be caught. ipc.ts itself is exempted below.
      "no-restricted-imports": [
        "error",
        {
          paths: [
            {
              name: "@tauri-apps/api/core",
              message:
                "Call backend commands only through src/lib/ipc.ts (the typed IPC boundary).",
            },
          ],
        },
      ],
    },
  },
  // The one sanctioned caller of @tauri-apps/api/core: ipc.ts wraps invoke/Channel for the
  // whole app, so the boundary rule above is turned back off for this single file.
  {
    files: ["src/lib/ipc.ts"],
    rules: {
      "no-restricted-imports": "off",
    },
  },
  // Node tooling (build/check scripts, config files).
  {
    files: ["scripts/**/*.mjs", "*.{js,mjs,ts}"],
    languageOptions: {
      globals: globals.node,
    },
  },
  prettier,
);
