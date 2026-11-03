// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import React, { lazy, Suspense } from "react";
import ReactDOM from "react-dom/client";
import { ThemeProvider, UserTimeProvider } from "./theme";
import { CapabilityProvider } from "./lib/capabilities";
import { ContextMenuHost, ErrorBoundary, TitleBar } from "./components/ui";
import { PopoverRoot } from "./PopoverRoot";
import "./theme/fonts";
import "./index.css";

// The always-on-top briefing window loads this same index.html with `?window=briefing`, so this is
// the fork between "the app" and "the one-card panel". `App` is lazy so Rollup splits it into its own
// chunk and the panel never downloads the main bundle just to show a paragraph of text.
//
// The pop-out Calendar (`?window=calendar`, popout.rs) mounts the Calendar tab on its own instead of
// App, with the same providers and title bar. Each is its own chunk, so neither window downloads the
// other's.
const windowKind = new URLSearchParams(window.location.search).get("window");
const isBriefingWindow = windowKind === "briefing";
const isCalendarWindow = windowKind === "calendar";
const App = lazy(() => (isCalendarWindow ? import("./CalendarWindow") : import("./App")));

ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
  <React.StrictMode>
    {/* PM's right-click menu, in every window: the browser's own page menu (Back, Reload) would
        reset the app. */}
    <ContextMenuHost />
    {isBriefingWindow ? (
      <PopoverRoot />
    ) : (
      <ThemeProvider>
        <UserTimeProvider>
          <CapabilityProvider>
            {/* Custom window chrome sits above everything (incl. App's loading/onboarding screens)
                so the frameless window can always be dragged/closed. */}
            <div className="flex h-full flex-col">
              <TitleBar subtitle={isCalendarWindow ? "Calendar" : undefined} />
              <div className="min-h-0 flex-1">
                {/* INSIDE the providers (the fallback's Button reads useTheme, which throws without
                    one) and OUTSIDE Suspense, so a failed `./App` chunk load is caught too — today
                    that is a permanently blank, unclosable frameless window. TitleBar stays outside
                    by construction, so the window can always be dragged and closed. This also
                    covers App's pre-view screens and its overlays, which the boundary inside App
                    never sees. */}
                <ErrorBoundary what={isCalendarWindow ? "The calendar" : "Personal Manager"}>
                  <Suspense fallback={null}>
                    <App />
                  </Suspense>
                </ErrorBoundary>
              </div>
            </div>
          </CapabilityProvider>
        </UserTimeProvider>
      </ThemeProvider>
    )}
  </React.StrictMode>,
);
