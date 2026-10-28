// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Where a dragged event would go (#884, C16b): drawn over the grid while the button is down, in the
// event's own colour with a dashed edge, its title and its new time, so what is let go is what was
// seen. A time the clocks skip draws it with the blocked edge and says why. It is only a picture:
// hidden from screen readers (the live region says what happens) and never under the pointer, so
// the drag's hit tests and the card's click go on as if it weren't there. The view places it.

import type { CSSProperties } from "react";
import type { CalendarEvent } from "../../../lib/types";
import { DRAFT_EVENT_CLASS } from "../../../lib/calendar-layout";
import { eventColour, eventFill, inkOn } from "../../../theme/eventPalette";
import { cn } from "../../ui";

/** The ghost the gesture shows, from its plan. */
export interface GhostView {
  /** The event drawn where it would go (`ghostEvent`), or the new event's draft for a create. */
  event: CalendarEvent;
  kind: "move" | "resize" | "create";
  /** Why letting go here does nothing (the clocks skip this time), or null. */
  invalid: string | null;
  /** When it would be, in words (`whenText`). */
  label: string;
}

interface Props {
  ghost: GhostView;
  /** The event's calendar colour: the left rule, and the fill for one without its own colour. */
  color: string;
  /** Its box in the column or the strip. */
  box: Pick<CSSProperties, "top" | "height" | "left" | "width">;
  /** The all-day strip's one-line form. */
  band?: boolean;
  /** A piece of an event across days: square and open where it continues. */
  continuesBefore?: boolean;
  continuesAfter?: boolean;
}

export function DragGhost({
  ghost,
  color,
  box,
  band = false,
  continuesBefore,
  continuesAfter,
}: Props) {
  const own = eventColour(ghost.event.color_id);
  const create = ghost.kind === "create";
  const style: CSSProperties = {
    ...box,
    borderLeft: `3px solid ${color}`,
    background: eventFill(own, color, create ? 18 : 28, "var(--surface)"),
    // The measured 3:1 edge (--ink3), or the blocked one where it can't go. A create takes the
    // draft's own dashed edge from its class.
    ...(create
      ? {}
      : {
          outline: `1.5px dashed ${ghost.invalid ? "var(--st-blocked)" : "var(--ink3)"}`,
          outlineOffset: "-1.5px",
        }),
    ...(continuesBefore ? { borderTopLeftRadius: 0, borderTopRightRadius: 0 } : {}),
    ...(continuesAfter ? { borderBottomLeftRadius: 0, borderBottomRightRadius: 0 } : {}),
  };
  const title = ghost.event.summary.trim() || "(no title)";
  return (
    <div
      aria-hidden="true"
      data-drag-ghost=""
      className={cn(
        "pointer-events-none absolute z-10 overflow-hidden rounded-[var(--radius-sm)] px-1.5",
        band ? "text-[0.6875rem] leading-[1.125rem]" : "py-0.5 shadow-md",
        create && DRAFT_EVENT_CLASS,
      )}
      style={style}
    >
      <div
        className={cn("truncate font-head font-medium text-ink", !band && "text-[0.6875rem]")}
        style={inkOn(own)}
      >
        {band ? `${title} · ${ghost.label}` : title}
      </div>
      {!band && (
        <div className="truncate font-mono text-[0.5625rem] text-ink2" style={inkOn(own)}>
          {ghost.label}
        </div>
      )}
      {ghost.invalid && (
        <div
          className="whitespace-normal font-ui text-[0.625rem] leading-tight text-ink"
          style={inkOn(own)}
        >
          {ghost.invalid}
        </div>
      )}
    </div>
  );
}
