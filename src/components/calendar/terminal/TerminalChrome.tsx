// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Terminal system's calendar status strip: a mono `~/pm ❯ cal --<view>` line that reinforces the
// CLI feel below the shared header. Purely informational — the CalendarHeader above it still owns the
// real nav/toggle controls; this only echoes the current view/range/period. Neutral inks only: green
// stays reserved for today/now inside the views, so nothing here (prompt, flags) is ever the accent.

import type { CalendarViewMode } from "../../../lib/calendarPrefs";
import { useDepth } from "../../../theme";

interface Props {
  view: CalendarViewMode;
  /** The current period label, e.g. "July 2026". */
  label: string;
  /** Visible (non-hidden) event count, surfaced at Power depth. */
  count: number;
}

export function TerminalChrome({ view, label, count }: Props) {
  const { showPower } = useDepth();
  return (
    <div className="flex items-center gap-2 border-b border-rule bg-panel px-4 py-1 font-mono text-xs">
      <span className="text-ink4">~/pm</span>
      <span aria-hidden className="text-ink3">
        ❯
      </span>
      <span className="text-ink2">cal --{view}</span>
      <span className="ml-auto truncate text-ink3">{label}</span>
      {showPower && (
        <span className="shrink-0 text-ink4">
          · {count} event{count === 1 ? "" : "s"}
        </span>
      )}
    </div>
  );
}
