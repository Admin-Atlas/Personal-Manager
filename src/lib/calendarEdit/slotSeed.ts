// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A new event's fields for a slot clicked or dragged on the calendar grid (#884, C17): C10's seed for
// a new event, with the slot's own times. Pure, so the times quick create opens on are the ones the
// grid showed under the pointer.

import { toMoveTo, type Timing } from "../calendarDrag";
import { DEFAULT_LENGTH_MS, newEventSeed, type EditorFields } from "./eventDraft";

/** A new event over `t`, a slot on the grid in this computer's wall time (`zone`, which creating on
 *  the grid needs). Every other field is a new event's (C10's seed, with C12's and C13's defaults);
 *  `now` is the seed's own clock. A timed slot is Busy and an all-day one Free, as Google creates them
 *  (C10's `switchNewKind` rule). Its length is already in `t`: a click took the create calendar's
 *  default length at the press. */
export function slotSeed(t: Timing, now: Date, zone: string): EditorFields {
  const at = toMoveTo(t);
  if (at.kind === "all_day") {
    return {
      ...newEventSeed(at.first_day, now, zone, DEFAULT_LENGTH_MS),
      time: { kind: "all_day", first_day: at.first_day, last_day: at.last_day },
      show_as: "free",
    };
  }
  return {
    ...newEventSeed(at.start_date, now, zone, DEFAULT_LENGTH_MS),
    time: {
      kind: "timed",
      start_date: at.start_date,
      start_time: at.start_time,
      start_zone: zone,
      end_date: at.end_date,
      end_time: at.end_time,
      end_zone: zone,
    },
    show_as: "busy",
  };
}
