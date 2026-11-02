// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// "+ New event" (#884, C10): the one control on the calendar's chrome that changes a calendar. It
// opens the editor for a new Google event, as `c` does; Shift+C's quick create opens under it
// (`createRef`). When no calendar can take an event it stays focusable and says why (its tooltip, and
// a click), rather than going grey and silent. It's secondary, so the editor's or quick create's Save
// stays the one primary button on screen.
//
// It has two homes, one per view and never both at once. In Week and Day it sits in the grid's
// top-left corner, above the timezone control, where there's room for it (`compact`: "+ New", and a
// click's reason drops below it, since the corner has no width to say it beside). In Month, Year,
// Agenda and the Terminal views there is no corner, so it sits at the header's right end.

import { useEffect, useId, useState, type Ref } from "react";
import { Button, Tooltip, VisuallyHidden, cn } from "../ui";

interface Props {
  onCreate: () => void;
  /** Why no new event can be made now, or null when one can. */
  createReason: string | null;
  /** Quick create opens under this button, and gives focus back to it. */
  createRef?: Ref<HTMLButtonElement>;
  /** The grid corner's narrow form. */
  compact?: boolean;
}

/** How long a click's "why not" stays beside the button. */
const SAID_MS = 6000;

export function NewEventButton({ onCreate, createReason, createRef, compact = false }: Props) {
  const reasonId = useId();
  // A click while none can be made: the reason, said for a moment (a tooltip shows only on hover or
  // focus, and a click doesn't focus a button in WebKit). Counted (0 = not said), so another click
  // starts the moment over and says it again as new text.
  const [said, setSaid] = useState(0);
  useEffect(() => {
    if (said === 0) return;
    const t = setTimeout(() => setSaid(0), SAID_MS);
    return () => clearTimeout(t);
  }, [said]);
  useEffect(() => {
    if (createReason === null) setSaid(0);
  }, [createReason]);

  return (
    <div className={cn("flex items-center gap-2", compact && "relative")} data-no-create="">
      <Tooltip label={createReason ?? "New event · c, or Shift+C for a quick one"}>
        <Button
          ref={createRef}
          variant="secondary"
          size={compact ? "xs" : "sm"}
          aria-disabled={createReason !== null || undefined}
          aria-describedby={createReason !== null ? reasonId : undefined}
          aria-keyshortcuts="c"
          // The same name in the corner, where only "+ New" fits on screen.
          aria-label={compact ? "+ New event" : undefined}
          className="aria-disabled:cursor-not-allowed aria-disabled:text-ink4"
          onClick={() => {
            if (createReason !== null) setSaid((n) => n + 1);
            else onCreate();
          }}
        >
          {compact ? "+ New" : "+ New event"}
        </Button>
      </Tooltip>
      {createReason !== null && (
        <>
          <VisuallyHidden id={reasonId}>{createReason}</VisuallyHidden>
          <span
            role="status"
            className={cn(
              "text-xs text-ink3",
              compact ? "absolute left-0 top-full z-20 mt-1 w-56" : "max-w-xs",
            )}
          >
            {/* A new node each click: a screen reader says text added, not text unchanged. */}
            {said > 0 && (
              <span
                key={said}
                className={cn(
                  compact &&
                    "block rounded-[var(--radius-sm)] border border-border bg-panel px-2 py-1 shadow-md",
                )}
              >
                {createReason}
              </span>
            )}
          </span>
        </>
      )}
    </div>
  );
}
