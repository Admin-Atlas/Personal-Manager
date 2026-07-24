// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A low-frequency "current time" tick shared across the app. Returns a Date that refreshes on an
// interval (default once a minute) so time-derived UI advances without an unrelated re-render — the
// calendar now-line / "today" rollover at 60s, the progress-bar elapsed timer at 1s. Cheap: one timer,
// no per-frame work.

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
