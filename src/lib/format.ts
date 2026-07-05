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
