// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { ThemeProvider } from "./theme";
import { CapabilityProvider } from "./lib/capabilities";
import { TitleBar } from "./components/ui";
import "./theme/fonts";
import "./index.css";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    <ThemeProvider>
      <CapabilityProvider>
        {/* Custom window chrome sits above everything (incl. App's loading/onboarding screens)
            so the frameless window can always be dragged/closed. */}
        <div className="flex h-full flex-col">
          <TitleBar />
          <div className="min-h-0 flex-1">
            <App />
          </div>
        </div>
      </CapabilityProvider>
    </ThemeProvider>
  </React.StrictMode>,
);
