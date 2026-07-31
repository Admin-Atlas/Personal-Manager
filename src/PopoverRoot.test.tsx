// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The briefing window's root. `main.tsx` forks on `?window=briefing` and renders THIS instead of the
// tree containing `<App/>`, so anything App mounts inline is simply absent here — which is how the
// external-link interceptor came to cover one of PM's two webview roots. The symptom was invisible:
// a `target="_blank"` the webview has no handler for is swallowed, so every link in the always-on-top
// briefing (including the bare-URL autolinks remark-gfm makes out of model prose) was dead with no
// error and no feedback.
//
// This is the case that fails if the hook is ever unmounted from this root again. The briefing body
// is stubbed with the one thing that matters about it — an anchor of the shape `<Markdown>` produces
// — so the assertion is about PopoverRoot's own wiring rather than about the briefing's contents.

import { cleanup, render, screen } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { PopoverRoot } from "./PopoverRoot";
import { openUrl } from "./lib/ipc";

vi.mock("./lib/ipc", () => ({
  openUrl: vi.fn(async () => undefined),
  closeBriefingWindow: vi.fn(async () => undefined),
  showMainWindow: vi.fn(async () => undefined),
}));

// The providers each reach for IPC of their own (theme prefs, user settings, the briefing itself);
// none of that is what this test is about, so they are reduced to pass-throughs. Each factory
// declares its own, because `vi.mock` is hoisted above every module-level binding.
type Wrap = { children?: ReactNode };
vi.mock("./theme", () => ({
  ThemeProvider: ({ children }: Wrap) => <>{children}</>,
  UserTimeProvider: ({ children }: Wrap) => <>{children}</>,
}));
vi.mock("./lib/briefing", () => ({
  BriefingProvider: ({ children }: Wrap) => <>{children}</>,
}));
vi.mock("./components/ui", () => ({
  ErrorBoundary: ({ children }: Wrap) => <>{children}</>,
}));
vi.mock("./components/ui/useEdgeResizeCursor", () => ({ useEdgeResizeCursor: () => {} }));
vi.mock("./components/Briefing", () => ({
  Briefing: () => (
    <a href="https://example.com/x" target="_blank" rel="noreferrer">
      a link in the briefing
    </a>
  ),
}));

const opened = vi.mocked(openUrl);

beforeEach(() => {
  opened.mockClear();
});

afterEach(cleanup);

describe("the briefing window's root", () => {
  it("intercepts a link in the briefing and hands it to the OS browser", () => {
    render(<PopoverRoot />);
    const ev = new MouseEvent("click", { bubbles: true, cancelable: true, button: 0 });
    screen.getByText("a link in the briefing").dispatchEvent(ev);
    expect(opened).toHaveBeenCalledWith("https://example.com/x");
    // And the window itself must not be left to act on the navigation.
    expect(ev.defaultPrevented).toBe(true);
  });

  it("still renders its own chrome", () => {
    render(<PopoverRoot />);
    // The drag strip is this frameless window's only way to be moved or closed, so a regression that
    // takes it out has to be loud.
    expect(screen.getByLabelText("Open Personal Manager")).toBeTruthy();
    expect(screen.getByLabelText("Close the briefing window")).toBeTruthy();
  });
});
