// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The "Calendars" dropdown: every synced calendar, grouped by account, each a source-colour dot +
// checkbox + name. Toggling show/hide is instant LOCAL view state (never re-syncs or purges — see
// calendarPrefs) and keeps the popover open for multi-select. It shows only *visibility*; which
// calendars sync at all is `calendars.selected`, owned by Connectors settings.

import { useMemo } from "react";
import type { Calendar, CalendarAccount } from "../../lib/types";
import { Button, cn } from "../ui";
import { Popover } from "./Popover";
import { SourceDot } from "./parts/SourceDot";

interface Props {
  accounts: CalendarAccount[];
  calendars: Calendar[];
  /** Calendar ids the user has hidden from the view. */
  hidden: Set<string>;
  onToggle: (calendarId: string) => void;
  colorOf: (calendarId: string) => string;
}

interface Group {
  key: string;
  label: string;
  calendars: Calendar[];
}

export function CalendarsDropdown({ accounts, calendars, hidden, onToggle, colorOf }: Props) {
  const groups = useMemo<Group[]>(() => {
    const labelFor = new Map(accounts.map((a) => [a.id, a.email || a.label] as const));
    const bySource = new Map<string, Calendar[]>();
    for (const c of calendars) {
      const list = bySource.get(c.source_id);
      if (list) list.push(c);
      else bySource.set(c.source_id, [c]);
    }
    return [...bySource.entries()].map(([sourceId, cals]) => ({
      key: sourceId,
      label: labelFor.get(sourceId) ?? "(calendar)",
      calendars: cals,
    }));
  }, [accounts, calendars]);

  const visibleCount = calendars.filter((c) => !hidden.has(c.id)).length;

  return (
    <Popover
      align="right"
      panelClassName="max-h-80 overflow-y-auto"
      trigger={({ open, toggle }) => (
        <Button
          variant="secondary"
          onClick={toggle}
          aria-expanded={open}
          title="Show or hide calendars"
        >
          Calendars
          <span className="font-mono text-xs text-ink4">
            {visibleCount}/{calendars.length}
          </span>
        </Button>
      )}
    >
      {calendars.length === 0 ? (
        <p className="px-2 py-2 text-xs text-ink4">No calendars connected.</p>
      ) : (
        groups.map((g) => (
          <div key={g.key} className="mb-1 last:mb-0">
            <p className="truncate px-2 pb-0.5 pt-1 font-mono text-[10px] uppercase tracking-wide text-faint">
              {g.label}
            </p>
            <ul>
              {g.calendars.map((c) => {
                const shown = !hidden.has(c.id);
                return (
                  <li key={c.id}>
                    <label className="flex cursor-pointer items-center gap-2 rounded-[var(--radius-sm)] px-2 py-1 text-sm text-ink hover:bg-surface">
                      <input
                        type="checkbox"
                        checked={shown}
                        onChange={() => onToggle(c.id)}
                        className="accent-[var(--accent)]"
                      />
                      <SourceDot color={colorOf(c.id)} className={cn(!shown && "opacity-40")} />
                      <span className={cn("truncate", !shown && "text-ink4")}>{c.name}</span>
                    </label>
                  </li>
                );
              })}
            </ul>
          </div>
        ))
      )}
    </Popover>
  );
}
