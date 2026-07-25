// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import React, { lazy, Suspense } from "react";
import ReactDOM from "react-dom/client";
import { ThemeProvider, UserTimeProvider } from "./theme";
import { CapabilityProvider } from "./lib/capabilities";
import { TitleBar } from "./components/ui";
import { PopoverRoot } from "./PopoverRoot";
import "./theme/fonts";
import "./index.css";

// The always-on-top briefing window loads this same index.html with `?window=briefing`, so this is
// the fork between "the app" and "the one-card panel". `App` is lazy so Rollup splits it into its own
// chunk and the panel never downloads the main bundle just to show a paragraph of text.
const App = lazy(() => import("./App"));

const isBriefingWindow = new URLSearchParams(window.location.search).get("window") === "briefing";

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {isBriefingWindow ? (
      <PopoverRoot />
    ) : (
      <ThemeProvider>
        <UserTimeProvider>
          <CapabilityProvider>
            {/* Custom window chrome sits above everything (incl. App's loading/onboarding screens)
                so the frameless window can always be dragged/closed. */}
            <div className="flex h-full flex-col">
              <TitleBar />
              <div className="min-h-0 flex-1">
                <Suspense fallback={null}>
                  <App />
                </Suspense>
              </div>
            </div>
          </CapabilityProvider>
        </UserTimeProvider>
      </ThemeProvider>
    )}
  </React.StrictMode>,
);
