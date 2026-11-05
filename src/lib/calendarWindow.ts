// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Which stretch of the mirror a calendar surface reads (#1047). PM keeps every event now — history
// back to each calendar's first event, and the months after the live band once the view has loaded
// them — so a view asks for the months around what it shows instead of for everything, and moves
// that stretch only when what it shows nears an edge. Pure, so it's tested without a store.

import type { CalendarOverview } from "./types";

/** A stretch of local months, `[start, end)`: both the first of a month, at local midnight. */
export interface MonthWindow {
  start: Date;
  end: Date;
}

/** The first of `at`'s month moved by `months`, at local midnight. */
function monthStart(at: Date, months = 0): Date {
  return new Date(at.getFullYear(), at.getMonth() + months, 1);
}

/** The months around `at`: `back` before its month and `ahead` after it (its own month included in
 *  neither count, so the window is `back + 1 + ahead` months long). */
export function windowAround(at: Date, back: number, ahead: number): MonthWindow {
  return { start: monthStart(at, -back), end: monthStart(at, ahead + 1) };
}

/** `current`, while `at` stays at least `margin` months inside it; otherwise a new window around
 *  `at`, so paging a little never re-reads and paging far reads the stretch ahead before it shows. */
export function nextWindow(
  current: MonthWindow,
  at: Date,
  back: number,
  ahead: number,
  margin: number,
): MonthWindow {
  const t = at.getTime();
  if (
    t >= monthStart(current.start, margin).getTime() &&
    t < monthStart(current.end, -margin).getTime()
  ) {
    return current;
  }
  return windowAround(at, back, ahead);
}

/** The read for a window: a day of slack either side, as the mirror reads an all-day date as UTC
 *  midnight while the window is local. */
export function windowRead(w: MonthWindow): { from: string; to: string } {
  const day = 86_400_000;
  return {
    from: new Date(w.start.getTime() - day).toISOString(),
    to: new Date(w.end.getTime() + day).toISOString(),
  };
}

/** A UTC month's key, `YYYY-MM`. */
function monthKey(year: number, month: number): string {
  const d = new Date(Date.UTC(year, month, 1));
  return `${d.getUTCFullYear()}-${String(d.getUTCMonth() + 1).padStart(2, "0")}`;
}

/** The months of `w` after the live band's end `mirrorEnd` (the first of a month, UTC midnight, as
 *  the backend gives it) that haven't been loaded yet (`loaded`, by `YYYY-MM`), as one stretch to
 *  load (`from`/`to` RFC 3339, UTC month starts) and the months it holds; `null` when there's none.
 *  A month between two loaded ones is loaded again with them, so the request stays one stretch. */
export function laterToLoad(
  w: MonthWindow,
  mirrorEnd: string,
  loaded: ReadonlySet<string>,
): { from: string; to: string; months: string[] } | null {
  const bandEnd = new Date(mirrorEnd);
  if (Number.isNaN(bandEnd.getTime())) return null;
  // The window's months as UTC months, padded one each side for zones far from UTC.
  const first = new Date(Date.UTC(w.start.getFullYear(), w.start.getMonth() - 1, 1));
  const last = new Date(Date.UTC(w.end.getFullYear(), w.end.getMonth() + 1, 1));
  const from = first > bandEnd ? first : bandEnd;
  const months: string[] = [];
  for (
    let y = from.getUTCFullYear(), m = from.getUTCMonth();
    Date.UTC(y, m, 1) < last.getTime();
    m += 1
  ) {
    months.push(monthKey(y, m));
  }
  const missing = months.filter((k) => !loaded.has(k));
  if (missing.length === 0) return null;
  const start = missing[0];
  const end = missing[missing.length - 1];
  const at = (key: string, plus: number) => {
    const [y, m] = key.split("-").map(Number);
    return new Date(Date.UTC(y, m - 1 + plus, 1)).toISOString();
  };
  return {
    from: at(start, 0),
    to: at(end, 1),
    months: months.filter((k) => k >= start && k <= end),
  };
}

/** How loading the months after the live band last went, for the view's hint. */
export type LaterLoad = "idle" | "loading" | "failed";

/** What the calendar view says when what it shows `visible` (local, `end` exclusive) isn't all in
 *  yet: the months after the live band still loading (`later`), or not reached; or, before the band,
 *  a calendar's history still downloading. Otherwise nothing: PM keeps every event (#1047). */
export function rangeHintText(
  overview: Pick<CalendarOverview, "mirror_start" | "mirror_end" | "history_ready">,
  visible: { start: Date; end: Date },
  later: LaterLoad,
): string | null {
  const side = beyondLiveBand(
    visible.start,
    visible.end,
    overview.mirror_start,
    overview.mirror_end,
  );
  if (side.after && later === "loading") return "Loading these months from your calendars…";
  if (side.after && later === "failed") {
    return "Couldn't load these months from your calendars. Check your connection, then press Refresh.";
  }
  if (side.before && !overview.history_ready) {
    return "Your older events are still downloading, so some may be missing here for a little while.";
  }
  return null;
}

/** Local midnight of `at`'s day. */
function dayOf(at: Date): number {
  return new Date(at.getFullYear(), at.getMonth(), at.getDate()).getTime();
}

/** Whether the visible `[start, end)` (local; `end` exclusive) reaches before the live band's start
 *  or past its end. Compared by LOCAL day: the band's bounds are UTC midnights, so a raw comparison
 *  would trip on a month whose every day is in the band whenever the zone isn't UTC. */
export function beyondLiveBand(
  start: Date,
  end: Date,
  mirrorStart: string,
  mirrorEnd: string,
): { before: boolean; after: boolean } {
  const ms = new Date(mirrorStart);
  const me = new Date(mirrorEnd);
  if (Number.isNaN(ms.getTime()) || Number.isNaN(me.getTime())) {
    return { before: false, after: false };
  }
  const lastVisible = new Date(end.getTime() - 86_400_000);
  return { before: dayOf(start) < dayOf(ms), after: dayOf(lastVisible) > dayOf(me) };
}
