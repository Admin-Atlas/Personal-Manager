// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A single-day event chip in the Month grid: a source-tinted pill with the title (and, in Power
// depth, its start time). The fill and left rule are the per-source colour mixed into transparency
// via color-mix, so the categorical hue arrives as a prop and no source hex is written in a component.
// An event given its own colour in Google fills with that, solid, the rule keeping its calendar's
// (`own`). Multi-day events render as bands (AllDayBand-style), not chips; Min depth collapses chips
// to dots. On a Month that drags (C16c) it carries its event's id for the drag's hit test; the drag
// itself lives in the calendar's gesture.

import { cn } from "../../ui";
import {
  DRAFT_EVENT_CLASS,
  DRAG_ORIGIN_CLASS,
  PAST_EVENT_CLASS,
  PENDING_EVENT_CLASS,
  spokenName,
} from "../../../lib/calendar-layout";
import { declinedName } from "../../../lib/calendarEdit/rsvp";
import { eventFill, eventHover, inkOn, type EventColour } from "../../../theme/eventPalette";

interface Props {
  summary: string;
  color: string;
  /** The colour the event was given in Google, when it has one: the fill (else `color`). */
  own?: EventColour | null;
  /** Local clock label for a timed event, e.g. "09:30"; empty for all-day. */
  timeLabel: string;
  /** Depth gate: show the time prefix (Power). */
  showTime: boolean;
  /** The event has fully passed — grey it back so what's done recedes. */
  isPast?: boolean;
  /** The new event being made (its ghost): the words its name starts with, saying so (`draftNote`).
   *  Drawn dashed, never greyed as past. Null for any other event. */
  ghost?: string | null;
  /** The user declined it: its title is struck through and its name says so, as Google draws it. */
  declined?: boolean;
  /** When set, the chip is interactive — click / Enter / Space opens the event's detail popup,
   *  anchored at the chip's on-screen rect. */
  onClick?: (anchor: DOMRect) => void;
  /** On a Month that drags (C16c): the event's id, which the drag reads to know what a press took.
   *  Absent elsewhere. */
  drag?: {
    eventId: string;
    /** It is being dragged: dimmed where it was. */
    dimmed: boolean;
    /** A move of it is being asked about or saved: drawn where it was dropped, dashed. */
    pending: boolean;
  };
}

export function EventChip({
  summary,
  color,
  own = null,
  timeLabel,
  showTime,
  isPast,
  ghost = null,
  declined = false,
  onClick,
  drag,
}: Props) {
  return (
    <div
      className={cn(
        "flex items-center gap-1 overflow-hidden rounded-[var(--radius-sm)] border-l-[2px] px-1 py-px text-[0.6875rem] leading-tight",
        onClick && `cursor-pointer ${eventHover(own)}`,
        // One look, so only one opacity utility is ever on the chip.
        ghost !== null
          ? DRAFT_EVENT_CLASS
          : drag?.dimmed
            ? DRAG_ORIGIN_CLASS
            : drag?.pending
              ? PENDING_EVENT_CLASS
              : isPast && PAST_EVENT_CLASS,
      )}
      data-draft-event={ghost !== null ? "" : undefined}
      data-event-id={drag?.eventId}
      style={{
        background: eventFill(own, color, 16),
        borderLeftColor: color,
      }}
      title={summary}
      aria-label={declinedName(
        spokenName(ghost, [timeLabel, summary].filter(Boolean).join(", ")),
        declined,
      )}
      role={onClick ? "button" : undefined}
      tabIndex={onClick ? 0 : undefined}
      onClick={onClick ? (e) => onClick(e.currentTarget.getBoundingClientRect()) : undefined}
      onKeyDown={
        onClick
          ? (e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                onClick(e.currentTarget.getBoundingClientRect());
              }
            }
          : undefined
      }
    >
      {/* --ink2, not --ink4: small text on the calendar's tint needs it for 4.5:1. */}
      {showTime && timeLabel && (
        <span className="shrink-0 font-mono text-[0.5625rem] text-ink2" style={inkOn(own)}>
          {timeLabel}
        </span>
      )}
      <span
        className={cn("truncate font-head text-ink", declined && "line-through")}
        style={inkOn(own)}
      >
        {summary}
      </span>
    </div>
  );
}
