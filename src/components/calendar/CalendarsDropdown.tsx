// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The "Calendars" dropdown: every synced calendar, grouped by account, each a source-colour dot +
// checkbox + name. Toggling show/hide is instant LOCAL view state (never re-syncs or purges — see
// calendarPrefs) and keeps the popover open for multi-select. It shows only *visibility*; which
// calendars sync at all is `calendars.selected`, owned by Connectors settings.

import { useMemo } from "react";
import type { Calendar, CalendarAccount } from "../../lib/types";
import { MILESTONE_CALENDAR_ID, PINBOARD_CALENDAR_ID } from "../../lib/calendar-layout";
import { Button, cn } from "../ui";
import { Popover } from "../ui";
import { SourceDot } from "./parts/SourceDot";

interface Props {
  accounts: CalendarAccount[];
  calendars: Calendar[];
  /** Calendar ids the user has hidden from the view. */
  hidden: Set<string>;
  onToggle: (calendarId: string) => void;
  colorOf: (calendarId: string) => string;
  /** Per-source shape slot for the colour-blind axis. This dropdown is the legend, so it shows the
   *  same shape beside each calendar name that the grid dots use — that mapping is what makes the
   *  shaped dots readable. */
  shapeOf?: (calendarId: string) => number | undefined;
}

interface Group {
  key: string;
  label: string;
  calendars: Calendar[];
}

/** One first-party overlay row (milestones / pinboard): the same dot + checkbox + name as a synced
 *  calendar, but keyed on a pseudo-calendar id. The `hidden` set is generic over arbitrary ids, so
 *  these toggle through exactly the same path. */
function OverlayRow({
  id,
  label,
  hidden,
  onToggle,
  colorOf,
}: {
  id: string;
  label: string;
  hidden: Set<string>;
  onToggle: (calendarId: string) => void;
  colorOf: (calendarId: string) => string;
}) {
  const shown = !hidden.has(id);
  return (
    <li>
      <label className="flex cursor-pointer items-center gap-2 rounded-[var(--radius-sm)] px-2 py-1 text-sm text-ink hover:bg-surface">
        <input
          type="checkbox"
          checked={shown}
          onChange={() => onToggle(id)}
          className="accent-[var(--accent)]"
        />
        <SourceDot color={colorOf(id)} className={cn(!shown && "opacity-40")} />
        <span className={cn("truncate", !shown && "text-ink4")}>{label}</span>
      </label>
    </li>
  );
}

export function CalendarsDropdown({
  accounts,
  calendars,
  hidden,
  onToggle,
  colorOf,
  shapeOf,
}: Props) {
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
      ariaLabel="Calendars to show"
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
      {/* The first-party overlays — pseudo-calendars you can show/hide like any synced one. Shown even
          with no calendars connected, since milestones and pinboard entries exist independently. */}
      <div className="mb-1">
        <p className="truncate px-2 pb-0.5 pt-1 font-mono text-[10px] uppercase tracking-wide text-faint">
          Personal Manager
        </p>
        <ul>
          <OverlayRow
            id={MILESTONE_CALENDAR_ID}
            label="Milestones"
            hidden={hidden}
            onToggle={onToggle}
            colorOf={colorOf}
          />
          <OverlayRow
            id={PINBOARD_CALENDAR_ID}
            label="Pinboard"
            hidden={hidden}
            onToggle={onToggle}
            colorOf={colorOf}
          />
        </ul>
      </div>
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
                      <SourceDot
                        color={colorOf(c.id)}
                        shapeIndex={shapeOf?.(c.id)}
                        className={cn(!shown && "opacity-40")}
                      />
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
