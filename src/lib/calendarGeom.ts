// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Pure geometry for the calendar's Work/Day/24h hour windows — kept free of React and the DOM (like
// calendar-layout.ts) so the sunrise/sunset rounding and the default/override resolution are easy to
// reason about and test. The time grid renders in the DEVICE-LOCAL zone, so the sunrise/sunset hours
// here are read from the local Date's own fields.

import { sunTimes } from "../theme/solar";
import type { Coords } from "../theme/timezones";
import {
  DAY_FALLBACK,
  FULL_BOUNDS,
  WORK_DEFAULT,
  type CalendarRange,
  type RangeBounds,
} from "./calendarPrefs";

function clamp(n: number, lo: number, hi: number): number {
  return Math.max(lo, Math.min(hi, n));
}

/** A local Date's clock position as a decimal hour-of-day (0..24). */
function hourOfDay(d: Date): number {
  return d.getHours() + d.getMinutes() / 60;
}

/**
 * The Day range's default window from the local sunrise→sunset for `date` at `coords`: sunrise floored
 * and sunset ceiled to the whole hour (an inclusive daytime frame that only shifts a few times a year,
 * not daily). Returns null when there's no location or the sun doesn't rise/set that day (polar), so
 * the caller falls back to {@link DAY_FALLBACK}.
 */
export function sunriseSunsetBounds(date: Date, coords: Coords | null): RangeBounds | null {
  if (!coords) return null;
  const t = sunTimes(date, coords[0], coords[1]);
  if (t.alwaysUp || t.alwaysDown || !t.sunrise || !t.sunset) return null;
  const startHour = clamp(Math.floor(hourOfDay(t.sunrise)), 0, 23);
  const endHour = clamp(Math.ceil(hourOfDay(t.sunset)), 1, 24);
  if (endHour - startHour < 1) return null;
  return { startHour, endHour };
}

/**
 * The effective visible-hour window for a range: a user override if set, else the range's default —
 * Work = 08:30–17:30, 24h = 00–24, Day = local sunrise→sunset (rounded) or {@link DAY_FALLBACK}.
 */
export function resolveRangeBounds(
  range: CalendarRange,
  custom: Partial<Record<CalendarRange, RangeBounds>>,
  coords: Coords | null,
  date: Date,
): RangeBounds {
  const override = custom[range];
  if (override) return override;
  if (range === "work") return WORK_DEFAULT;
  if (range === "full") return FULL_BOUNDS;
  return sunriseSunsetBounds(date, coords) ?? DAY_FALLBACK;
}
