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
 * Format an ISO timestamp as the same DD-MM(-YYYY) date plus a locale time
 * (hour:minute). Leaves an unparseable value as-is.
 */
export function formatDateTime(iso: string): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  const time = d.toLocaleTimeString(undefined, {
    hour: "2-digit",
    minute: "2-digit",
  });
  return `${formatDate(iso)} ${time}`;
}
