// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A low-frequency "current time" tick for the calendar. Returns a Date that refreshes on an interval
// (default once a minute) so the now-line advances and the "today" highlight rolls over at midnight
// without needing an unrelated re-render. Cheap: one timer, no per-frame work.

import { useEffect, useState } from "react";

/** A `Date` that updates every `periodMs` (default 60s). */
export function useNowTick(periodMs = 60_000): Date {
  const [now, setNow] = useState(() => new Date());
  useEffect(() => {
    const id = setInterval(() => setNow(new Date()), periodMs);
    return () => clearInterval(id);
  }, [periodMs]);
  return now;
}
