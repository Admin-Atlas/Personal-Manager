// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The pure text<->ISO layer behind `DateField`, PM's replacement for `<input type="date">`.
//
// WHY THIS EXISTS AT ALL: WebKitGTK (Linux) implements no date-input widget the way Blink does, and
// the popup it does show swallows outside clicks until Escape. Beyond that, `type="date"` renders in
// the *OS* locale, so those fields never obeyed the app's DD-MM-YYYY rule. Both problems disappear
// once PM owns the control — but only if the text half is forgiving, because a typed date is now the
// primary input path on Linux rather than a fallback nobody reaches.
//
// Forgiving means: accept `-`, `/` or `.` as the separator, accept one- or two-digit day/month,
// accept a two-digit year, infer the current year when it is omitted entirely, and accept a pasted
// ISO `YYYY-MM-DD` unchanged (that is what the vault stores, and what a user copying out of PM has on
// their clipboard). Rejection is reserved for values that are genuinely not a date — never for a
// value that is merely mid-typing, which is the exact trap that made the Work-Day hour field
// untypable on Linux (see the `webkitgtk-native-input-degradation` note).

/** The canonical stored shape: a date-only `YYYY-MM-DD`, or "" for no date. */
export type IsoDate = string;

/** Zero-pad to two digits. (Local copy rather than importing `format.ts` — this module is the pure
 *  parse layer and stays dependency-free so it can be reasoned about, and tested, on its own.) */
function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

/** Whether y/m/d name a real calendar day — rejects 31-02, 30-02, 31-04 and friends. Built by
 *  round-tripping through `Date`, which normalises an overflowing day into the next month. */
export function isRealDate(year: number, month1: number, day: number): boolean {
  if (month1 < 1 || month1 > 12 || day < 1 || day > 31) return false;
  const d = new Date(year, month1 - 1, day);
  return d.getFullYear() === year && d.getMonth() === month1 - 1 && d.getDate() === day;
}

/** Build `YYYY-MM-DD` from calendar fields (no `Date` round-trip, so no timezone shift). */
export function toIso(year: number, month1: number, day: number): IsoDate {
  return `${String(year).padStart(4, "0")}-${pad2(month1)}-${pad2(day)}`;
}

/** A local `Date`'s own calendar day as `YYYY-MM-DD`. Deliberately NOT `toISOString().slice(0,10)`,
 *  which converts to UTC and lands a day early west of Greenwich (F-14). */
export function dateToIso(d: Date): IsoDate {
  return Number.isNaN(d.getTime()) ? "" : toIso(d.getFullYear(), d.getMonth() + 1, d.getDate());
}

/** Today as `YYYY-MM-DD`, in the user's own timezone. `now` injectable for tests. */
export function todayIso(now: Date = new Date()): IsoDate {
  return dateToIso(now);
}

/** Parse a stored `YYYY-MM-DD` into a local `Date` at midnight, or null. Used to seed the picker's
 *  month and its selected day; `new Date("2026-08-14")` would read as UTC midnight and can show the
 *  previous day as selected in a UTC-negative zone. */
export function isoToDate(iso: IsoDate): Date | null {
  const m = /^(\d{4})-(\d{2})-(\d{2})$/.exec(iso);
  if (!m) return null;
  const [y, mo, d] = [Number(m[1]), Number(m[2]), Number(m[3])];
  return isRealDate(y, mo, d) ? new Date(y, mo - 1, d) : null;
}

/**
 * Stored ISO → what the user sees and edits: always full `DD-MM-YYYY`.
 *
 * Deliberately does NOT drop the year in the current year the way {@link formatDateOnly} does. That
 * elision is right for a *label* and wrong for an *editable field*: the displayed text is the text
 * the user edits and re-parses, so hiding a component makes the round-trip lossy and "14-08" would
 * silently mean something different next January.
 */
export function isoToDisplay(iso: IsoDate): string {
  const d = isoToDate(iso);
  return d ? `${pad2(d.getDate())}-${pad2(d.getMonth() + 1)}-${d.getFullYear()}` : "";
}

/** Two-digit years map into the current century's ±50-year window: 26 → 2026, 99 → 1999 when read
 *  from 2026. Four-digit years are taken as written. */
function expandYear(raw: string, now: Date): number {
  const n = Number(raw);
  if (raw.length > 2) return n;
  const century = Math.floor(now.getFullYear() / 100) * 100;
  const candidate = century + n;
  return candidate - now.getFullYear() > 50 ? candidate - 100 : candidate;
}

/**
 * User text → stored ISO.
 *
 * Returns `""` for an empty field (an intentional clear — a milestone with no deadline is valid),
 * `null` for text that is not a date, and `YYYY-MM-DD` otherwise. The `"" vs null` split is the whole
 * point: the caller must be able to tell "the user cleared this" from "the user typed nonsense", and
 * only the second one should be refused.
 *
 * Accepted: `14-08-2026`, `14/8/26`, `14.08.2026`, `14-08` (current year), and a pasted ISO
 * `2026-08-14`. `now` is injectable so the year-inference boundary is testable.
 */
export function parseDisplay(text: string, now: Date = new Date()): IsoDate | null {
  const s = text.trim();
  if (s === "") return "";

  // A pasted ISO date, taken as-is — it is unambiguous and it is what PM stores.
  const iso = /^(\d{4})-(\d{1,2})-(\d{1,2})$/.exec(s);
  if (iso) {
    const [y, mo, d] = [Number(iso[1]), Number(iso[2]), Number(iso[3])];
    return isRealDate(y, mo, d) ? toIso(y, mo, d) : null;
  }

  // Day-first, the app's own order. The year is optional; the separator may be - / or . but must be
  // used consistently, so "14-08/2026" is rejected rather than half-understood.
  const m = /^(\d{1,2})([-/.])(\d{1,2})(?:\2(\d{2,4}))?$/.exec(s);
  if (!m) return null;
  const day = Number(m[1]);
  const month = Number(m[3]);
  const year = m[4] ? expandYear(m[4], now) : now.getFullYear();
  return isRealDate(year, month, day) ? toIso(year, month, day) : null;
}
