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

/**
 * A last-synced stamp for a compact control: the clock time if the sync happened today, else the
 * DD-MM date. Day-aware on purpose — a bare `HH:MM` for a sync that last succeeded yesterday reads as
 * today. That is harmless as a faint meta line off to one side, but actively misleading once it is
 * the label on the very button you press to refresh. `now` is injectable so the boundary is testable.
 */
export function formatSyncedShort(iso: string, now: Date = new Date()): string {
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "";
  const sameDay =
    d.getFullYear() === now.getFullYear() &&
    d.getMonth() === now.getMonth() &&
    d.getDate() === now.getDate();
  return sameDay ? formatClock(d) : formatDateLocal(d);
}

/** A model id trimmed to its bare name for compact display ("meta-llama/Llama-3-8B" → "Llama-3-8B"):
 *  drops the provider/namespace prefix before the first slash. Shared by the sidebar model rows and
 *  the chat provenance footer / fallback strip. */
export function shortModel(id: string): string {
  const slash = id.indexOf("/");
  return slash >= 0 ? id.slice(slash + 1) : id;
}

/** 2^30 — the base every `*_gb` figure crossing the IPC boundary is already in (`hardware.rs`'s
 *  `GIB`, `local_disk.rs::bytes_to_gb`, the catalog's `file_gb`), and `fit.rs` compares those three
 *  against each other. A GB float is converted BACK to bytes here, never re-scaled. */
const GIB = 1024 ** 3;

const BYTE_UNITS = ["B", "KB", "MB", "GB", "TB"] as const;

/**
 * A byte count as a human size. **Binary** steps (1 KB = 1024 B) under the short SI labels, because
 * that is what the machine says: Windows Explorer and Task Manager, macOS's memory readout, and PM's
 * own fit maths all count in powers of 1024. Decimal here made the same model read "4.7 GB" while it
 * downloaded and "4.3 GB" the instant it landed on disk — a same-screen contradiction.
 *
 * One decimal place from GB up, whole numbers below. Nullish/non-finite render as an em dash; zero
 * and below as "0 B" (the old version returned the literal string "NaN undefined" for a negative).
 *
 * This is the ONE byte formatter. Three others existed — `StorageSettings.formatSize`, and
 * LocalAiSettings' `fmtGb`/`fmtBytes` — and the decimal one was the defect. {@link formatGib} is the
 * adapter for a figure the backend already divided; `formatSize` survives only as the `~`-prefix
 * copy decision wrapped around this.
 */
export function formatBytes(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n)) return "—";
  if (n <= 0) return "0 B";
  // Round FIRST, then promote, so 1 048 575 B is "1 MB" and never "1024 KB".
  const show = (v: number, i: number) => (i >= 3 ? Number(v.toFixed(1)) : Math.round(v));
  let i = 0;
  let v = n;
  while (i < BYTE_UNITS.length - 1 && show(v, i) >= 1024) {
    v /= 1024;
    i += 1;
  }
  return `${show(v, i).toFixed(i >= 3 ? 1 : 0)} ${BYTE_UNITS[i]}`;
}

/** {@link formatBytes} for a figure the backend already expressed in GB — the `*_gb` family (RAM,
 *  VRAM, free disk, a model's weights). Goes back through {@link GIB} rather than re-deriving a
 *  scale, so a nearly-full disk reads "410 MB" instead of the old "0.4 GB", and a big volume reads
 *  "1.5 TB" instead of "1500.0 GB". Unchanged at every ordinary value: 16 → "16.0 GB". */
export function formatGib(gb: number | null | undefined): string {
  return gb == null || !Number.isFinite(gb) ? "—" : formatBytes(gb * GIB);
}
