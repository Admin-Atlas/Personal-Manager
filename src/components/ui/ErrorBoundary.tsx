// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The app's one error boundary — and, because React offers no hook equivalent, its one class
// component. Without it a single render throw unmounts the entire tree, which in a FRAMELESS window
// means a blank rectangle with no title bar: nothing left to close, move, or navigate with.
//
// PLACEMENT RULE — never mount this OUTSIDE <ThemeProvider>. The fallback renders `Button`, which
// reads the active System through `useTheme()`, and that THROWS without the provider: a boundary
// above it would fail inside its own fallback and blank the root anyway, which is strictly worse
// than no boundary. For the same reason window chrome (TitleBar, the briefing's drag strip) stays
// OUTSIDE every boundary — a render throw must never be able to take the only way to close a window
// with it.
//
// No reset prop, deliberately: callers reset by changing React's own `key` (App keys this on the
// current view, so clicking anything in the sidebar mounts a clean instance), which is why the card
// offers only Reload and no "try again".
//
// Scope, stated plainly so nobody reads more into it: a boundary catches RENDER-phase throws only.
// Event handlers, promise rejections, timers and async IPC are not caught — App already `.catch()`es
// its IPC calls, so this closes the blank-window class, not "all errors".

import { Component, type ErrorInfo, type ReactNode } from "react";
import { Button } from "./Button";
import { Card } from "./Card";
import { Collapsible } from "./Collapsible";

export interface ErrorBoundaryProps {
  children: ReactNode;
  /** What stopped working, in the card's own words — e.g. "This view", "The briefing". */
  what?: string;
}

interface State {
  error: Error | null;
  /** React's component stack: the only part that says WHERE it broke. */
  stack: string | null;
}

export class ErrorBoundary extends Component<ErrorBoundaryProps, State> {
  state: State = { error: null, stack: null };

  static getDerivedStateFromError(error: Error): Partial<State> {
    return { error };
  }

  componentDidCatch(_error: Error, info: ErrorInfo): void {
    // Kept in state rather than logged. `src/` has no console calls at all, React 19 already logs
    // the error itself, and PM is local-first — the person who has to report this bug is the person
    // looking at the card, so the stack belongs on screen, folded away, not in a console they will
    // never open.
    this.setState({ stack: info.componentStack ?? null });
  }

  render(): ReactNode {
    const { error, stack } = this.state;
    // Bare children, NOT a wrapper element: App's content column is a flex chain
    // (`relative flex flex-1` → `<main className="flex h-full flex-1 flex-col">`), and one stray
    // div in the middle of it silently changes every view's layout.
    if (!error) return this.props.children;

    const what = this.props.what ?? "This view";
    const message = String(error.message || error);
    return (
      <div role="alert" className="flex h-full min-w-0 flex-1 items-center justify-center p-6">
        <Card className="max-w-md p-5 text-sm">
          <p className="font-head text-base text-ink">{what} stopped working</p>
          <p className="mt-2 text-ink3">
            Nothing was lost — your vault is untouched. Pick another section in the sidebar, or
            reload PM.
          </p>
          <Collapsible className="mt-3" title="Technical details" defaultOpen={false}>
            <p className="mt-2 font-mono text-xs break-words text-ink3">{message}</p>
            {stack !== null && (
              <pre className="mt-2 max-h-48 overflow-auto font-mono text-xs whitespace-pre-wrap text-ink4">
                {stack}
              </pre>
            )}
          </Collapsible>
          <Button className="mt-4" variant="primary" onClick={() => window.location.reload()}>
            Reload PM
          </Button>
        </Card>
      </div>
    );
  }
}
