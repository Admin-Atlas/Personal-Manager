// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

/** Zero-pad a number to at least two digits. */
function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

/**
 * Format an ISO timestamp as DD-MM-YYYY, dropping the year when the date falls
 * in the current year. Leaves an unparseable value as-is.
 */
export function formatDate(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const day = pad2(d.getDate());
  const month = pad2(d.getMonth() + 1);
  const year = d.getFullYear();
  return year === new Date().getFullYear() ? `${day}-${month}` : `${day}-${month}-${year}`;
}

/**
 * Format a DATE-ONLY value (`YYYY-MM-DD`, or an ISO timestamp whose calendar date is what matters) as
 * DD-MM(-YYYY), built from the y/m/d fields directly. {@link formatDate} runs the string through
 * `new Date`, which reads a bare `YYYY-MM-DD` as UTC **midnight** — a day early in UTC-negative zones
 * (F-14). This takes the date components straight, so a milestone / deadline / all-day date renders on
 * its own calendar day everywhere. A value that isn't a leading `YYYY-MM-DD` falls back to
 * {@link formatDate} (which handles full timestamps and leaves junk as-is).
 */
export function formatDateOnly(value: string): string {
  const m = /^(\d{4})-(\d{2})-(\d{2})/.exec(value);
  if (!m) return formatDate(value);
  return formatDateLocal(new Date(Number(m[1]), Number(m[2]) - 1, Number(m[3])));
}

/**
 * Like {@link formatDate} but from a local `Date`'s own calendar fields — no ISO round-trip, which
 * would shift the day across timezones (a local day stringified to UTC can land a day earlier).
 * DD-MM, dropping the year in the current year. Used by the calendar view, which works in local days.
 */
export function formatDateLocal(d: Date): string {
  if (Number.isNaN(d.getTime())) return "";
  const day = pad2(d.getDate());
  const month = pad2(d.getMonth() + 1);
  const year = d.getFullYear();
  return year === new Date().getFullYear() ? `${day}-${month}` : `${day}-${month}-${year}`;
}

/**
 * A local `Date`'s clock time as locale `HH:MM`. Empty string for an invalid date. Shared by the
 * calendar views (time-grid, month chips, both agendas) so the one clock format never drifts.
 */
export function formatClock(d: Date): string {
  if (Number.isNaN(d.getTime())) return "";
  return d.toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit" });
}

/** {@link formatClock} from an ISO/date string; empty string if unparseable. */
export function formatClockIso(iso: string): string {
  return formatClock(new Date(iso));
}

/**
 * Format an ISO timestamp as the same DD-MM(-YYYY) date plus a locale time
 * (hour:minute). Leaves an unparseable value as-is.
 */
export function formatDateTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return `${formatDate(iso)} ${formatClock(d)}`;
}

/**
 * A connector's "last synced" timestamp as a full local wall-clock string (date + time). Deliberately
 * the OS locale format ({@link Date.toLocaleString}), NOT the app's DD-MM-YYYY — a returning user wants
 * the exact moment the last sync ran. Leaves an unparseable value as-is. Shared by the index-only
 * connectors (Drive / OneDrive / local folders).
 */
export function formatWhen(iso: string): string {
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? iso : d.toLocaleString();
}
