// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// One timed event in the pixel time-grid (Week/Day), or one day's piece of an event that runs across
// days (square and open at the ends where it continues). Absolutely positioned by the geometry the view
// computes from calendar-layout; the card itself only paints. The fill is a shade of the event's own
// source colour mixed into the surface via color-mix (token-safe — no source hex written here), so a
// timed block reads as its calendar's colour like the all-day bands do, instead of a flat neutral that
// vanished into the grid. Opaque-over-surface (not …, transparent) on purpose: timed cards overlap and
// sit over the today-column tint, where a translucent fill goes muddy. An event given its own colour in
// Google is filled with that instead, solid, in its own text colour, the stripe keeping its calendar's
// colour, as Google draws it (`own`). Meta reveals with depth AND available height (a 20px sliver has
// no room for a time line); a past event is greyed via isPast. On a grid that drags (C16b) it carries
// its id and day for the drag's hit test, and an invisible strip with a resize cursor at each end
// that may be dragged; the drag itself lives in the calendar's gesture, not here.

import type { CSSProperties } from "react";
import { cn } from "../../ui";
import {
  DRAFT_EVENT_CLASS,
  DRAG_ORIGIN_CLASS,
  PAST_EVENT_CLASS,
  PENDING_EVENT_CLASS,
  spokenName,
} from "../../../lib/calendar-layout";
import { handleSizes } from "../../../lib/calendarDrag";
import { declinedName } from "../../../lib/calendarEdit/rsvp";
import { eventFill, eventHover, inkOn, type EventColour } from "../../../theme/eventPalette";

interface Props {
  summary: string;
  /** The calendar's source colour (categorical palette) — the left rule. */
  color: string;
  /** The colour the event was given in Google, when it has one: the fill (else the calendar's). */
  own?: EventColour | null;
  /** Local clock label, e.g. "09:30–10:30", or for an event across days its dates and times. */
  timeLabel: string;
  location: string | null;
  /** Absolute geometry within the day column. */
  topPx: number;
  heightPx: number;
  leftPct: number;
  widthPct: number;
  /** Depth gate for the time line (also needs a tall-enough card). */
  showTime: boolean;
  /** Depth gate for the location line (Power, and only on tall cards). */
  showLocation: boolean;
  /** The event has fully passed — grey it back so what's done recedes. */
  isPast?: boolean;
  /** The new event being made (its ghost): the words its name starts with, saying so (`draftNote`).
   *  Drawn dashed, never greyed as past. Null for any other event. */
  ghost?: string | null;
  /** The user declined it: its title is struck through and its name says so, as Google draws it. */
  declined?: boolean;
  /** A piece of an event across days: it started on an earlier day (square top) or runs on into
   *  a later one (square bottom). */
  continuesBefore?: boolean;
  continuesAfter?: boolean;
  /** Whether Tab stops here (default yes). The later pieces of an event across days are still
   *  clickable, but only its first piece on screen is a tab stop. */
  tabStop?: boolean;
  /** Open the event's detail popup, anchored at the card's on-screen rect. */
  onSelect?: (anchor: DOMRect) => void;
  /** On a grid that drags (C16b): the event's id and this piece's day (`YYYY-MM-DD`), which the
   *  drag reads to know what a press took. Absent elsewhere (Focus's read-only grid). */
  drag?: {
    eventId: string;
    day: string;
    /** Which ends may be dragged on this piece: the top on the event's first piece, the bottom on
     *  its last, where moving is allowed. */
    edges: { start: boolean; end: boolean };
    /** It is being dragged: dimmed where it was. */
    dimmed: boolean;
    /** A move of it is being asked about or saved: drawn where it was dropped, dashed. */
    pending: boolean;
  };
}

// Height thresholds below which a line has no room — keep them out of the render so a squeezed card
// stays just its title rather than overflowing.
const TIME_MIN_H = 38;
const LOC_MIN_H = 52;

export function EventCard({
  summary,
  color,
  own = null,
  timeLabel,
  location,
  topPx,
  heightPx,
  leftPct,
  widthPct,
  showTime,
  showLocation,
  isPast,
  ghost = null,
  declined = false,
  continuesBefore,
  continuesAfter,
  tabStop = true,
  onSelect,
  drag,
}: Props) {
  const style: CSSProperties = {
    top: `${topPx}px`,
    height: `${heightPx}px`,
    left: `calc(${leftPct}% + 1px)`,
    width: `calc(${widthPct}% - 2px)`,
    borderLeftColor: color,
    background: eventFill(own, color, 18, "var(--surface)"),
    // A piece of an event across days runs edge to edge into the next day's piece: square, open
    // ends where it continues (inline, as `cn` doesn't resolve conflicting utilities).
    ...(continuesBefore
      ? { borderTopWidth: 0, borderTopLeftRadius: 0, borderTopRightRadius: 0 }
      : {}),
    ...(continuesAfter
      ? { borderBottomWidth: 0, borderBottomLeftRadius: 0, borderBottomRightRadius: 0 }
      : {}),
  };
  const withTime = showTime && heightPx >= TIME_MIN_H;
  const withLoc = showLocation && !!location && heightPx >= LOC_MIN_H;

  // A full accessible name (summary + time + place) even when the card is too short to show the meta,
  // the ghost's saying first that it's the new event, and a declined one's saying so last.
  const ariaLabel = declinedName(
    spokenName(ghost, [summary, timeLabel, location].filter(Boolean).join(", ")),
    declined,
  );

  // The edges that may be dragged (`handleSizes`): a press on one of these strips takes that edge.
  const handles = handleSizes(heightPx);

  return (
    <div
      className={cn(
        "absolute overflow-hidden rounded-[var(--radius-sm)] border border-border border-l-[3px] px-1.5 py-0.5",
        onSelect && `cursor-pointer ${eventHover(own)}`,
        // One look, so only one opacity utility is ever on the card.
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
      data-day={drag?.day}
      style={style}
      title={summary}
      aria-label={ariaLabel}
      role={onSelect ? "button" : undefined}
      tabIndex={onSelect ? (tabStop ? 0 : -1) : undefined}
      onClick={onSelect ? (e) => onSelect(e.currentTarget.getBoundingClientRect()) : undefined}
      onKeyDown={
        onSelect
          ? (e) => {
              if (e.key === "Enter" || e.key === " ") {
                e.preventDefault();
                onSelect(e.currentTarget.getBoundingClientRect());
              }
            }
          : undefined
      }
    >
      <div
        className={cn(
          "truncate font-head text-[0.6875rem] font-medium leading-tight text-ink",
          declined && "line-through",
        )}
        style={inkOn(own)}
      >
        {summary}
      </div>
      {/* --ink2, not --ink4: small text on the calendar's tint needs it for 4.5:1. */}
      {withTime && (
        <div className="truncate font-mono text-[0.5625rem] text-ink2" style={inkOn(own)}>
          {timeLabel}
        </div>
      )}
      {withLoc && (
        <div className="truncate font-mono text-[0.5625rem] text-ink2" style={inkOn(own)}>
          {location}
        </div>
      )}
      {drag?.edges.start && handles.top > 0 && (
        <div
          aria-hidden="true"
          data-edge="start"
          className="absolute inset-x-0 top-0 cursor-ns-resize"
          style={{ height: `${handles.top}px` }}
        />
      )}
      {drag?.edges.end && handles.bottom > 0 && (
        <div
          aria-hidden="true"
          data-edge="end"
          className="absolute inset-x-0 bottom-0 cursor-ns-resize"
          style={{ height: `${handles.bottom}px` }}
        />
      )}
    </div>
  );
}
