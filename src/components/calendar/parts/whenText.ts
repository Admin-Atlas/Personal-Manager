// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A mirrored event's "when" in words: the event popover's line, and the delete dialog's. Moved out of
// CalendarEventPopover (unchanged) so both can say it the same way.

import type { CalendarEvent } from "../../../lib/types";
import { formatClock, formatDateLocal } from "../../../lib/format";
import { parseLocal } from "../../../lib/calendar-layout";

/** A human "when" line: an all-day date (or range), or a date + start–end clock. */
export function whenText(ev: CalendarEvent): string {
  const start = parseLocal(ev.start, ev.all_day);
  if (!start) return ev.start;
  if (ev.all_day) {
    const end = ev.end ? parseLocal(ev.end, true) : null;
    // All-day end is exclusive; show a range only when it spans more than the single start day.
    if (end && end.getTime() - 86_400_000 > start.getTime()) {
      const last = new Date(end.getTime() - 86_400_000);
      return `All day · ${formatDateLocal(start)} – ${formatDateLocal(last)}`;
    }
    return `All day · ${formatDateLocal(start)}`;
  }
  const end = ev.end ? parseLocal(ev.end, false) : null;
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
