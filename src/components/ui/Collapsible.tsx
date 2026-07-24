// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A collapsible/expandable section — a header (with a disclosure caret + an optional
// trailing meta slot) over a body that animates open/closed. This is the "result /
// stream card" vocabulary from the design (§15.3): wire it to real, present content
// (ingest results, citations) today; it's ready for agent reasoning streams in v4.
//
// `defaultOpen` is uncontrolled-with-a-seed: callers pass it from useDepth() (e.g. the
// connector groups use `!minimal` — collapsed at "min", open above it) so density drives
// the initial state without the primitive itself reaching into theme. That seeding is for
// bodies of *controls*; a body of pure explanation goes through `SectionInfo`, which starts
// closed at every depth and takes no `defaultOpen` at all. The body animates via the grid
// 0fr↔1fr trick (no height measurement); the transition is dropped under prefers-reduced-motion.

import { useState, type ReactNode } from "react";
import { useTheme } from "../../theme";
import { cn } from "./cn";

export interface CollapsibleProps {
  title: ReactNode;
  /** Initial open state (callers usually derive this from useDepth). Default open. */
  defaultOpen?: boolean;
  /** Trailing header slot — e.g. a count or a status. */
  meta?: ReactNode;
  children: ReactNode;
  className?: string;
}

export function Collapsible({
  title,
  defaultOpen = true,
  meta,
  children,
  className,
}: CollapsibleProps) {
  const [open, setOpen] = useState(defaultOpen);
  const { system } = useTheme();
  return (
    <div className={cn("flex flex-col", className)}>
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen((o) => !o)}
        className="flex w-full items-center gap-2 text-left text-sm text-ink2 transition-colors hover:text-ink"
      >
        <span
          aria-hidden
          className="inline-block text-xs text-ink4 transition-transform duration-200 motion-reduce:transition-none"
          style={{ transform: open ? "rotate(90deg)" : "rotate(0deg)" }}
        >
          {system === "terminal" ? ">" : "▸"}
        </span>
        <span className="flex-1 font-medium">{title}</span>
        {meta != null && <span className="shrink-0 font-mono text-xs text-ink4">{meta}</span>}
      </button>
      <div
        className="grid transition-[grid-template-rows] duration-200 ease-out motion-reduce:transition-none"
        style={{ gridTemplateRows: open ? "1fr" : "0fr" }}
      >
        {/* `inert` when collapsed removes the (still-rendered, height-0) body from the tab order and
            the a11y tree — otherwise the grid 0fr trick hides it visually but keyboard focus and
            screen readers still reach it. React 19 drops the attribute when false. */}
        <div className="overflow-hidden" inert={!open}>
          {children}
        </div>
      </div>
    </div>
  );
}
