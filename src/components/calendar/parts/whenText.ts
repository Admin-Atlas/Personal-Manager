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
