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
  /** The flag printed after `cal --`, when it isn't the view's own name: `runway` for the Agenda's
   *  Runway layout. */
  flag?: string;
  /** The current period label, e.g. "July 2026". */
  label: string;
  /** Visible (non-hidden) event count, surfaced at Power depth. */
  count: number;
  /** What `count` counts: events, or (on the Runway) open tasks. */
  noun?: "event" | "task";
}

export function TerminalChrome({ view, flag, label, count, noun = "event" }: Props) {
  const { showPower } = useDepth();
  return (
    <div className="flex items-center gap-2 border-b border-rule bg-panel px-4 py-1 font-mono text-xs">
      <span className="text-ink4">~/pm</span>
      <span aria-hidden className="text-ink3">
        ❯
      </span>
      <span className="text-ink2">cal --{flag ?? view}</span>
      <span className="ml-auto truncate text-ink3">{label}</span>
      {showPower && (
        <span className="shrink-0 text-ink4">
          · {count} {noun}
          {count === 1 ? "" : "s"}
        </span>
      )}
    </div>
  );
}
