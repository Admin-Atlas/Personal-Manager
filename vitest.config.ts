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
export default defineConfig({
  plugins: [react()],
  test: {
    include: [
      "src/lib/**/*.test.ts",
      "src/lib/**/*.test.tsx",
      "src/components/**/*.test.tsx",
      "src/theme/**/*.test.ts",
    ],
    environment: "node",
  },
});
