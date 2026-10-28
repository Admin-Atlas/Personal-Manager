// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The all-day / multi-day strip above the time-grid body. Each band is packed into a stacked lane
// (calendar-layout.packBands) and positioned by day index across the visible columns, clamped to the
// window with flat edges where it continues off-screen. Fill/border are the per-source colour mixed
// into transparency via color-mix (token-safe — no source hex is written here); an event given its own
// colour in Google fills with that, solid, the border keeping its calendar's. Renders nothing when no
// band events fall in range, so the row collapses, except on a grid that drags (C16b), where it keeps
// one row to drop on.

import { useMemo } from "react";
import type { CalendarEvent } from "../../../lib/types";
import {
  clampSpanToRange,
  dayDiff,
  dayKey,
  DRAFT_EVENT_CLASS,
  DRAG_ORIGIN_CLASS,
  draftNote,
  eventDaySpan,
  isEventPast,
  packBands,
  PAST_EVENT_CLASS,
  PENDING_EVENT_CLASS,
  spokenName,
  type BandInput,
} from "../../../lib/calendar-layout";
import { HANDLE_MAX_PX, HANDLE_PARTS } from "../../../lib/calendarDrag";
import { declinedName, isDeclined } from "../../../lib/calendarEdit/rsvp";
import { eventColour, eventFill, eventHover, inkOn } from "../../../theme/eventPalette";
import { cn } from "../../ui";
import { DragGhost, type GhostView } from "./DragGhost";

interface Props {
  /** Band events only (multi-day, or single all-day) — the view pre-filters. */
  events: CalendarEvent[];
  /** The visible day columns (local midnights), left → right. */
  days: Date[];
  colorOf: (calendarId: string) => string;
  /** Left gutter width, matched to the time-grid's hour gutter so bands align with columns. */
  gutterPx: number;
  /** Right gutter matching the scrolling body's vertical-scrollbar width, so the band's columns line
   *  up with the time-grid below it (0 under overlay scrollbars). */
  endGutterPx?: number;
  /** Depth gate for the "all-day" gutter label. */
  showLabel: boolean;
  /** The ticking "now" (device-local) so a band whose last day is past greys back. Defaults to the
   *  render-time clock for embeds that don't thread a tick. */
  now?: Date;
  /** Open an event's detail popup, anchored at the band's on-screen rect. */
  onEventClick?: (ev: CalendarEvent, anchor: DOMRect) => void;
  /** On a grid that drags (C16b): the strip keeps one row with no all-day events (one appearing as
   *  a drag reaches it would shift the hours under the pointer), carries its days and each bar's id
   *  for the drag's hit test, and draws an all-day ghost in its first row. */
  drag?: BandDrag;
}

/** What a dragging grid tells the strip (C16b). */
export interface BandDrag {
  /** Whether an event's ends may be dragged here. */
  bandEdges: (ev: CalendarEvent) => boolean;
  /** The event being dragged: dimmed where it is. */
  originId: string | null;
  /** Moves being asked about or saved: drawn where they were dropped, dashed. */
  pendingIds: ReadonlySet<string>;
  /** Where the dragged event would go, when that's all day. */
  ghost: GhostView | null;
}

const LANE_H = 20;
/** A bar's draggable end: `bandHitZone`'s size, the handle's width or its share of the bar. */
const EDGE_W = `min(${HANDLE_MAX_PX}px, ${100 / HANDLE_PARTS}%)`;

interface Placed extends BandInput {
  ev: CalendarEvent;
  continuesLeft: boolean;
  continuesRight: boolean;
}

export function AllDayBand({
  events,
  days,
  colorOf,
  gutterPx,
  endGutterPx = 0,
  showLabel,
  now,
  onEventClick,
  drag,
}: Props) {
  const ndays = days.length;
  const nowRef = now ?? new Date();
  // The all-day ghost's place across the visible days, clamped like a bar.
  const dropGhost = drag?.ghost?.event.all_day ? drag.ghost : null;
  const ghostSpan = useMemo(() => {
    const span = dropGhost ? eventDaySpan(dropGhost.event) : null;
    if (!span || ndays === 0) return null;
    return clampSpanToRange(
      dayDiff(days[0], span.startDay),
      dayDiff(days[0], span.endDay),
      ndays - 1,
    );
  }, [dropGhost, days, ndays]);

  const { placed, laneCount } = useMemo(() => {
    if (ndays === 0) return { placed: [] as (Placed & { lane: number })[], laneCount: 0 };
    const anchor = days[0];
    const lastIndex = ndays - 1;
    const inputs: Placed[] = [];
    for (const ev of events) {
      const span = eventDaySpan(ev);
      if (!span) continue;
      const clamped = clampSpanToRange(
        dayDiff(anchor, span.startDay),
        dayDiff(anchor, span.endDay),
        lastIndex,
      );
      if (!clamped) continue;
      inputs.push({
        id: ev.id,
        startDay: clamped.startDay,
        endDay: clamped.endDay,
        continuesLeft: clamped.continuesLeft,
        continuesRight: clamped.continuesRight,
        ev,
      });
    }
    const { bands, laneCount } = packBands(inputs);
    const byId = new Map(inputs.map((i) => [i.id, i]));
    const placed = bands.map((b) => {
      const src = byId.get(b.id)!;
      return { ...src, lane: b.lane };
    });
    return { placed, laneCount };
  }, [events, days, ndays]);

  if (laneCount === 0 && !drag) return null;

  return (
    <div className="flex border-b border-rule" style={{ paddingRight: endGutterPx }}>
      <div
        className="flex shrink-0 items-start justify-end pr-2 pt-1"
        style={{ width: `${gutterPx}px` }}
      >
        {showLabel && <span className="font-mono text-[0.5625rem] text-ink4">all-day</span>}
      </div>
      <div
        className={cn("relative flex-1", drag && "select-none")}
        style={{ height: `${Math.max(laneCount, drag ? 1 : 0) * LANE_H}px` }}
        data-surface={drag ? "band" : undefined}
        data-days={drag ? days.map(dayKey).join(",") : undefined}
      >
        {placed.map((b) => {
          const color = colorOf(b.ev.calendar_id);
          const own = eventColour(b.ev.color_id);
          const leftPct = (b.startDay / ndays) * 100;
          const widthPct = ((b.endDay - b.startDay + 1) / ndays) * 100;
          const clickable = !!onEventClick;
          const past = isEventPast(b.ev, nowRef);
          const ghost = draftNote(b.ev);
          const edges = drag?.bandEdges(b.ev) ?? false;
          return (
            <div
              key={b.ev.id}
              className={cn(
                "absolute overflow-hidden px-1.5 text-[0.6875rem] leading-[1.125rem]",
                clickable && `cursor-pointer ${eventHover(own)}`,
                // One look, so only one opacity utility is ever on the bar.
                ghost !== null
                  ? DRAFT_EVENT_CLASS
                  : drag?.originId === b.ev.id
                    ? DRAG_ORIGIN_CLASS
                    : drag?.pendingIds.has(b.ev.id)
                      ? PENDING_EVENT_CLASS
                      : past && PAST_EVENT_CLASS,
              )}
              data-draft-event={ghost !== null ? "" : undefined}
              data-event-id={drag ? b.ev.id : undefined}
              style={{
                top: `${b.lane * LANE_H}px`,
                left: `${leftPct}%`,
                width: `${widthPct}%`,
                height: `${LANE_H - 2}px`,
                background: eventFill(own, color, 22),
                // The calendar's stripe at its true start, drawn inside the bar rather than as a
                // border: a border would push the left end's strip 3 px in from the edge the drag
                // measures from, so what looks like the end wouldn't be (#884, C16b review). The
                // text keeps its place beside it.
                boxShadow: b.continuesLeft ? undefined : `inset 3px 0 0 ${color}`,
                paddingLeft: b.continuesLeft ? undefined : "calc(0.375rem + 3px)",
                borderTopLeftRadius: b.continuesLeft ? 0 : "var(--radius-sm)",
                borderBottomLeftRadius: b.continuesLeft ? 0 : "var(--radius-sm)",
                borderTopRightRadius: b.continuesRight ? 0 : "var(--radius-sm)",
                borderBottomRightRadius: b.continuesRight ? 0 : "var(--radius-sm)",
              }}
              title={b.ev.summary}
              aria-label={declinedName(spokenName(ghost, b.ev.summary), isDeclined(b.ev))}
              role={clickable ? "button" : undefined}
              tabIndex={clickable ? 0 : undefined}
              onClick={
                clickable
                  ? (e) => onEventClick?.(b.ev, e.currentTarget.getBoundingClientRect())
                  : undefined
              }
              onKeyDown={
                clickable
                  ? (e) => {
                      if (e.key === "Enter" || e.key === " ") {
                        e.preventDefault();
                        onEventClick?.(b.ev, e.currentTarget.getBoundingClientRect());
                      }
                    }
                  : undefined
              }
            >
              <span
                className={cn("truncate font-head text-ink", isDeclined(b.ev) && "line-through")}
                style={inkOn(own)}
              >
                {b.continuesLeft ? "‹ " : ""}
                {b.ev.summary}
              </span>
              {/* The ends that may be dragged, at the event's true ends only (one cut at the
                  window's edge continues): as wide as the drag's hit test takes them. */}
              {edges && !b.continuesLeft && (
                <div
                  aria-hidden="true"
                  data-edge="left"
                  className="absolute inset-y-0 left-0 cursor-ew-resize"
                  style={{ width: EDGE_W }}
                />
              )}
              {edges && !b.continuesRight && (
                <div
                  aria-hidden="true"
                  data-edge="right"
                  className="absolute inset-y-0 right-0 cursor-ew-resize"
                  style={{ width: EDGE_W }}
                />
              )}
            </div>
          );
        })}
        {dropGhost && ghostSpan && (
          <DragGhost
            ghost={dropGhost}
            color={colorOf(dropGhost.event.calendar_id)}
            band
            box={{
              top: 0,
              height: `${LANE_H - 2}px`,
              left: `${(ghostSpan.startDay / ndays) * 100}%`,
              width: `${((ghostSpan.endDay - ghostSpan.startDay + 1) / ndays) * 100}%`,
            }}
          />
        )}
      </div>
    </div>
  );
}
