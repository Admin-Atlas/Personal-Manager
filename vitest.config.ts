// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { defineConfig } from "vitest/config";
import react from "@vitejs/plugin-react";

// T-07: the frontend test harness. Scoped to the pure, invariant-bearing modules in `src/lib` — the
// ones the audit flagged as guarded by nothing (date formatting, the markdown sanitize schema,
// calendar layout). A `node` environment (no jsdom) keeps the footprint minimal: the suites test pure
// functions, not rendered components. The React plugin is present only so a `.tsx` module that carries
// a pure helper (e.g. `markdown.tsx`'s `safeUrl`/`SCHEMA`) can be imported without its JSX failing to
// transform. Kept separate from `vite.config.ts` so the Tauri dev/build config stays untouched.
export default defineConfig({
  plugins: [react()],
  test: {
    include: ["src/lib/**/*.test.ts", "src/lib/**/*.test.tsx"],
    environment: "node",
  },
});
