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
  { ignores: ["dist", "dist-ssr", "src-tauri/target", "design-system-docs", "node_modules"] },
  js.configs.recommended,
  ...tseslint.configs.recommended,
  // Frontend (browser).
  {
    files: ["src/**/*.{ts,tsx}"],
    languageOptions: {
      ecmaVersion: 2022,
      globals: globals.browser,
    },
    plugins: {
      "react-hooks": reactHooks,
      "react-refresh": reactRefresh,
    },
    rules: {
      "react-hooks/rules-of-hooks": "error",
      "react-hooks/exhaustive-deps": "warn",
      "react-refresh/only-export-components": ["warn", { allowConstantExport: true }],
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
