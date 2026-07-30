// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// The frontend test harness. Started as T-07 (pure `src/lib` modules the audit flagged as guarded by
// nothing — date formatting, the markdown sanitize schema, calendar layout) and grew a Wave-4 jsdom
// layer for component/hook tests (first targets: `useDetachedSync` + the unified connector component).
// The DEFAULT environment stays `node` so the pure suites keep their tiny footprint; the handful of
// tests that need a DOM opt in per-file with `// @vitest-environment jsdom`, so jsdom is only ever
// loaded for those. Kept separate from `vite.config.ts` so the Tauri dev/build config stays untouched.
//
// It also covers `scripts/**/*.test.mjs` — the build scripts are plain Node and can ride this runner,
// so the catalog generator's correctness rules get tested without a second harness (and without the
// extra justfile recipe + pr.yml step a `node --test` suite would have needed).
export default defineConfig({
  plugins: [react()],
  test: {
    // One pattern for the whole source tree, not a list of directories. The list version was
    // narrower than the tree it was meant to cover — `src/components/**/*.test.tsx` collected no
    // `.test.ts`, `src/theme/**/*.test.ts` collected no `.test.tsx`, and a new `src/` subdirectory
    // would have been collected by nothing at all. A test file that is never collected reports
    // nothing and looks exactly like a passing one. `check-files-in-place.mjs` proves the tracked
    // set holds no test file this misses.
    include: ["src/**/*.test.{ts,tsx}", "scripts/**/*.test.mjs"],
    environment: "node",
  },
});
