// @vitest-environment jsdom
// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Three properties, each of which the app is unusable without.
//
// The boundary exists because PM's window is FRAMELESS: an uncaught render throw unmounts the whole
// tree, and what is left is a blank rectangle with no title bar — nothing to close or move. So the
// first case asserts the survival of a sibling rendered outside the boundary directly, rather than
// trusting the placement to stay right. The third pins the shape of the HEALTHY path, which is the
// easier one to break by accident: one wrapper div in there changes every view's flex layout and
// nothing fails except the look of the app.
//
// `useTheme` is stubbed so the fallback's Button/Collapsible don't need the full ThemeProvider
// (which pulls in IPC) — the same stub ConnectorItemRow.test.tsx uses.

import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

vi.mock("../../theme/ThemeContext", async (importOriginal) => ({
  ...(await importOriginal<object>()),
  useTheme: () => ({ system: "slate", mode: "dark", accent: "mono", depth: "standard" }),
}));

import { ErrorBoundary } from "./ErrorBoundary";

function Boom({ message = "chunk 12 is not a function" }: { message?: string }): never {
  throw new Error(message);
}

// React logs every caught error itself, so without this each case prints a full component stack and
// the real failures get lost in it.
let consoleError: ReturnType<typeof vi.spyOn>;
beforeEach(() => {
  consoleError = vi.spyOn(console, "error").mockImplementation(() => {});
});
afterEach(() => {
  consoleError.mockRestore();
  cleanup();
});

describe("ErrorBoundary", () => {
  it("shows a recovery card while everything outside it keeps rendering", () => {
    render(
      <>
        <div data-testid="chrome">window chrome</div>
        <ErrorBoundary what="This view">
          <Boom />
        </ErrorBoundary>
      </>,
    );

    expect(screen.getByRole("alert")).toBeTruthy();
    expect(screen.getByText("This view stopped working")).toBeTruthy();
    expect(screen.getByText("chunk 12 is not a function")).toBeTruthy();
    expect(screen.getByRole("button", { name: /Reload PM/ })).toBeTruthy();
    // The point of the whole exercise: the title bar is still there to close the window with.
    expect(screen.getByTestId("chrome")).toBeTruthy();
  });

  it("clears the error when the key changes, which is how navigation resets it", () => {
    // There is no `onReset` prop by design — App keys the boundary on the current view, so picking
    // anything in the sidebar mounts a clean instance. If that stopped working the card would be a
    // dead end with only Reload out of it.
    const { rerender } = render(
      <ErrorBoundary key="focus">
        <Boom />
      </ErrorBoundary>,
    );
    expect(screen.getByRole("alert")).toBeTruthy();

    rerender(
      <ErrorBoundary key="documents">
        <div data-testid="healthy">documents</div>
      </ErrorBoundary>,
    );
    expect(screen.queryByRole("alert")).toBeNull();
    expect(screen.getByTestId("healthy")).toBeTruthy();
  });

  it("adds no element of its own when the child is healthy", () => {
    // The regression guard for App's flex chain: the healthy path must return `children` bare.
    const { container } = render(
      <ErrorBoundary>
        <p data-testid="healthy">documents</p>
      </ErrorBoundary>,
    );
    expect(container.childNodes.length).toBe(1);
    expect(container.firstChild).toBe(screen.getByTestId("healthy"));
  });
});
