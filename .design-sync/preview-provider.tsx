// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Preview wrapper used by design-sync (cfg.provider). NOT shipped as a usable primitive — it exists
// so preview cards render the way the app actually looks: every primitive uses light-on-dark text
// designed to sit on the app's dark canvas (var(--bg)), but the preview card body is hardcoded white
// upstream. This paints the themed surface behind each preview AND supplies the required ThemeProvider
// context (primitives call useTheme()). Default theme = editorial · dark · orange accent.
import type { ReactNode } from "react";
import { ThemeProvider } from "../src/theme";

export function PreviewSurface({ children }: { children: ReactNode }) {
  return (
    <ThemeProvider>
      <div
        style={{
          background: "var(--bg)",
          color: "var(--ink)",
          fontFamily: "var(--ui)",
          padding: 18,
          borderRadius: 10,
          width: "100%",
          boxSizing: "border-box",
        }}
      >
        {children}
      </div>
    </ThemeProvider>
  );
}
