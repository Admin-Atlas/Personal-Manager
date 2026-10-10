// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A mirrored event's "when" in words: the event popover's line, and the delete dialog's. Moved out of
// CalendarEventPopover (unchanged) so both can say it the same way.

import type { CalendarEvent } from "../../../lib/types";
import { formatClock, formatDateLocal } from "../../../lib/format";
import { eventDaySpan, parseLocal } from "../../../lib/calendar-layout";

/** A human "when" line: an all-day date (or range), a date + start–end clock, or for a timed event
 *  that runs into another day, both dates ("10-10 22:00 – 11-10 02:00"). One ending at midnight
 *  keeps its one date, as it fills only its own day. Days are counted in calendar days, so the night
 *  the clocks go back (a 25-hour day) is still one day. */
export function whenText(ev: CalendarEvent): string {
  const start = parseLocal(ev.start, ev.all_day);
  if (!start) return ev.start;
  const span = eventDaySpan(ev);
  const acrossDays = !!span && span.endDay.getTime() > span.startDay.getTime();
  if (ev.all_day) {
    // All-day end is exclusive; `eventDaySpan` gives the last day itself.
    return acrossDays
      ? `All day · ${formatDateLocal(span.startDay)} – ${formatDateLocal(span.endDay)}`
      : `All day · ${formatDateLocal(start)}`;
  }
  const end = ev.end ? parseLocal(ev.end, false) : null;
  if (end && acrossDays) {
    return `${formatDateLocal(start)} ${formatClock(start)} – ${formatDateLocal(end)} ${formatClock(end)}`;
  }
  const clock = end ? `${formatClock(start)}–${formatClock(end)}` : formatClock(start);
  return `${formatDateLocal(start)} · ${clock}`;
}

/** How a repeating event repeats, in words ("Weekly on Monday"), or a plain "Repeats" when PM has
 *  no words for it. Until 3.144 an iCal row held its raw rule ("FREQ=WEEKLY;BYDAY=MO"); one that
 *  hasn't synced since still does, and reads as a plain "Repeats" until it has. */
export function repeatsText(ev: CalendarEvent): string {
  const summary = ev.recurrence_summary?.trim();
  return summary && !/^(RRULE:)?FREQ=/i.test(summary) ? summary : "Repeats";
}
