// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

/** Zero-pad a number to at least two digits. */
export function pad2(n: number): string {
  return String(n).padStart(2, "0");
}

/** A running elapsed duration as `m:ss`, or `h:mm:ss` once it passes an hour — the progress-bar
 *  timer. Negative/NaN inputs floor to `0:00`. */
export function formatElapsed(ms: number): string {
  const total = Number.isFinite(ms) ? Math.max(0, Math.floor(ms / 1000)) : 0;
  const s = total % 60;
  const m = Math.floor(total / 60) % 60;
  const h = Math.floor(total / 3600);
  return h > 0 ? `${h}:${pad2(m)}:${pad2(s)}` : `${m}:${pad2(s)}`;
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

/** A short "when" for a calendar event — a bare date for all-day, date + time otherwise. Shared by the
 *  Focus agenda list and the per-project card so the two read identically. All-day events carry a bare
 *  date, formatted from its own calendar day so it can't shift a day in a UTC-negative zone (F-14). */
export function formatEventWhen(start: string, allDay?: boolean): string {
  const d = new Date(start);
  if (Number.isNaN(d.getTime())) return start.slice(0, 16);
  if (allDay || !start.includes("T")) return formatDateOnly(start);
  return `${formatDate(start)} ${formatClock(d)}`;
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

/** A model id trimmed to its bare name for compact display ("meta-llama/Llama-3-8B" → "Llama-3-8B"):
 *  drops the provider/namespace prefix before the first slash. Shared by the sidebar model rows and
 *  the chat provenance footer / fallback strip. */
export function shortModel(id: string): string {
  const slash = id.indexOf("/");
  return slash >= 0 ? id.slice(slash + 1) : id;
}

/** A byte count as an exact human size ("1.4 GB"). (StorageSettings keeps its own `formatSize` —
 *  that one deliberately floors at MB and marks estimates with `~`.) */
export function formatBytes(n: number): string {
  if (!n) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  const i = Math.min(units.length - 1, Math.floor(Math.log(n) / Math.log(1024)));
  const v = n / Math.pow(1024, i);
  return `${v >= 100 || i === 0 ? Math.round(v) : v.toFixed(1)} ${units[i]}`;
}
