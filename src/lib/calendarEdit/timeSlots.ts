// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Times of day as the calendar's pickers offer them. Lifted out of RangeControl (the Work/Day hours
// editor) so the event editor's TimeField shares the vocabulary: 24h `HH:MM`, locale-independent,
// picked from a list rather than typed (WebKitGTK has no `type="time"` widget; see RangeControl).

/** Decimal hour → "HH:MM". 24 renders as "24:00" — end-of-day, which a time input can't express. */
export function hoursToHM(h: number): string {
  const hh = Math.floor(h);
  const mm = Math.round((h - hh) * 60);
  return `${String(hh).padStart(2, "0")}:${String(mm).padStart(2, "0")}`;
}

/** Half-hour slots over [lo, hi] inclusive — the granularity sanitizeBounds' round05 already pins. */
export function slots(lo: number, hi: number): number[] {
  const out: number[] = [];
  for (let h = lo; h <= hi + 1e-9; h += 0.5) out.push(Math.round(h * 2) / 2);
  return out;
}

/** Every quarter hour of a day, "00:00" to "23:45": the event editor's time choices. */
export const QUARTER_HOURS: readonly string[] = Array.from({ length: 96 }, (_, i) =>
  hoursToHM(i / 4),
);

/** Whether `value` is a 24h `HH:MM` time of day ("00:00"–"23:59"). */
export function isHM(value: string): boolean {
  return /^([01]\d|2[0-3]):[0-5]\d$/.test(value);
}

/** An event's length as Google's end-time list words it: "15 mins", "1 hr", "1.5 hrs", "2.25 hrs".
 *  Quarter hours read as decimals the way Google writes them; any other length is spelled out
 *  ("1 hr 10 mins"). Real elapsed time, so a night the clocks change reads true. */
export function durationLabel(ms: number): string {
  const mins = Math.round(ms / 60_000);
  if (mins < 60) return `${mins} ${mins === 1 ? "min" : "mins"}`;
  if (mins % 15 === 0) {
    const hrs = mins / 60;
    return `${hrs} ${hrs === 1 ? "hr" : "hrs"}`;
  }
  const h = Math.floor(mins / 60);
  const m = mins % 60;
  return `${h} ${h === 1 ? "hr" : "hrs"} ${m} ${m === 1 ? "min" : "mins"}`;
}

/** The editor's time choices with `value` and `held` (the time Google holds) among them: an event at
 *  09:10 must still show 09:10, and stay pickable after a slip to 09:15, so going back to it sends
 *  nothing. An off-grid time is slotted in where it sorts; anything that isn't `HH:MM` is left out. */
export function timeChoices(value: string, held?: string): string[] {
  const extra = [value, held].filter(
    (t): t is string => t !== undefined && isHM(t) && !QUARTER_HOURS.includes(t),
  );
  return [...new Set([...QUARTER_HOURS, ...extra])].sort();
}
