// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Wall-clock times in a named zone, without a date library: `Intl.DateTimeFormat#formatToParts` is
// the only source of zone rules the webview has. The zone is always passed in; nothing here reads
// the device zone, so the same date and time can be asked about in any zone and the answer never
// depends on where the computer is (no UTC fallback either, plan rule R6).
//
// It mirrors the backend's `calendar_write::time`, which has the final say on save: a time the clocks
// skip is refused (`gap`), and a time that happens twice takes its first occurrence (`ambiguous`,
// with both instants, so the editor can say which one it means).

/** A date and a time of day as the editor holds them: `YYYY-MM-DD` and 24h `HH:MM`. */
export interface WallParts {
  date: string;
  time: string;
}

export type WallResolution =
  | { kind: "ok"; instant: Date }
  /** The clocks pass it twice (the hour they go back): `instant` is the first, `later` the second. */
  | { kind: "ambiguous"; instant: Date; later: Date }
  /** The clocks skip it (the hour they go forward). */
  | { kind: "gap" }
  | { kind: "invalid"; what: "zone" | "date" | "time" };

const DAY_MS = 24 * 3600 * 1000;

const formatters = new Map<string, Intl.DateTimeFormat>();

/** One formatter per zone (building one is slow, and a week view asks hundreds of times). Throws
 *  for a zone `Intl` doesn't know. */
function formatter(zone: string): Intl.DateTimeFormat {
  let f = formatters.get(zone);
  if (!f) {
    f = new Intl.DateTimeFormat("en-GB", {
      timeZone: zone,
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
      hourCycle: "h23",
    });
    formatters.set(zone, f);
  }
  return f;
}

/** Whether `zone` is an IANA zone this webview knows (`theme`'s `isValidTimeZone`, with the formatter
 *  kept for the reads that follow). */
function zoneKnown(zone: string): boolean {
  if (!zone.trim()) return false;
  try {
    formatter(zone);
    return true;
  } catch {
    return false;
  }
}

/** The wall-clock fields of `ms` in `zone`, as numbers. */
function fields(ms: number, zone: string) {
  const parts = formatter(zone).formatToParts(new Date(ms));
  const get = (type: string) => Number(parts.find((p) => p.type === type)?.value);
  return {
    year: get("year"),
    month: get("month"),
    day: get("day"),
    // Some engines say "24" for midnight even under h23.
    hour: get("hour") % 24,
    minute: get("minute"),
    second: get("second"),
  };
}

/** How far `zone` is ahead of UTC at `ms`, in milliseconds. */
function offsetMs(ms: number, zone: string): number {
  const f = fields(ms, zone);
  const asUtc = Date.UTC(f.year, f.month - 1, f.day, f.hour, f.minute, f.second);
  return asUtc - Math.floor(ms / 1000) * 1000;
}

const pad = (n: number, width = 2) => String(n).padStart(width, "0");

/** `instant` as the date and time of day it reads in `zone` (seconds dropped). */
export function wallTimeOf(instant: Date, zone: string): WallParts {
  const f = fields(instant.getTime(), zone);
  return {
    date: `${pad(f.year, 4)}-${pad(f.month)}-${pad(f.day)}`,
    time: `${pad(f.hour)}:${pad(f.minute)}`,
  };
}

/** The instant(s) a wall date and time name in `zone`. */
export function resolveWallTime(date: string, time: string, zone: string): WallResolution {
  if (!zoneKnown(zone)) return { kind: "invalid", what: "zone" };
  const d = /^(\d{4})-(\d{2})-(\d{2})$/.exec(date);
  if (!d) return { kind: "invalid", what: "date" };
  const t = /^([01]\d|2[0-3]):([0-5]\d)$/.exec(time);
  if (!t) return { kind: "invalid", what: "time" };
  const [y, m, day] = [Number(d[1]), Number(d[2]), Number(d[3])];
  const naive = Date.UTC(y, m - 1, day, Number(t[1]), Number(t[2]));
  const check = new Date(naive);
  if (check.getUTCFullYear() !== y || check.getUTCMonth() !== m - 1 || check.getUTCDate() !== day) {
    return { kind: "invalid", what: "date" }; // 2026-02-30 and the like
  }
  // Every offset the zone uses within a day either side; a real zone changes at most once in that
  // span, so the wall time is one of these offsets away from the naive UTC reading, or none.
  const offsets = new Set([naive - DAY_MS, naive, naive + DAY_MS].map((ms) => offsetMs(ms, zone)));
  const hits = [...new Set([...offsets].map((o) => naive - o))]
    .filter((ms) => {
      const w = wallTimeOf(new Date(ms), zone);
      return w.date === date && w.time === time;
    })
    .sort((a, b) => a - b);
  if (hits.length === 0) return { kind: "gap" };
  if (hits.length === 1) return { kind: "ok", instant: new Date(hits[0]) };
  return { kind: "ambiguous", instant: new Date(hits[0]), later: new Date(hits[hits.length - 1]) };
}

/** The instant a wall time names, its first occurrence when the clocks pass it twice, or `null` when
 *  it doesn't exist (a skipped hour) or can't be read. */
export function wallInstant(date: string, time: string, zone: string): Date | null {
  const r = resolveWallTime(date, time, zone);
  return r.kind === "ok" || r.kind === "ambiguous" ? r.instant : null;
}
