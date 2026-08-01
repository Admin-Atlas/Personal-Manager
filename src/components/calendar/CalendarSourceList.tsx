// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The calendar visibility list: the first-party overlays, then every synced calendar grouped by
// account, each a source-colour dot + checkbox + name. Toggling is instant LOCAL view state (never
// re-syncs or purges — see calendarPrefs); it shows only *visibility*. Which calendars sync at all is
// `calendars.selected`, owned by Connectors settings.
//
// Extracted so the sidebar block and the header popover are literally the same rows rather than two
// implementations that drift. It is also the colour-blind legend — the shape beside each name is what
// makes the shaped dots in the grid readable — so `shapeOf` must be threaded wherever it is rendered.

import { useMemo } from "react";
import type { Calendar, CalendarAccount } from "../../lib/types";
import { MILESTONE_CALENDAR_ID, PINBOARD_CALENDAR_ID } from "../../lib/calendar-layout";
import { cn } from "../ui";
import { SourceDot } from "./parts/SourceDot";

export interface CalendarSourceListProps {
  accounts: CalendarAccount[];
  calendars: Calendar[];
  /** Calendar ids the user has hidden from the view. */
  hidden: Set<string>;
  onToggle: (calendarId: string) => void;
  colorOf: (calendarId: string) => string;
  shapeOf?: (calendarId: string) => number | undefined;
  /** Hide calendars that aren't selected in Connectors. They mirror no events and can never show
   *  one, which is far more conspicuous inline than tucked inside a popover. */
  hideUnselected?: boolean;
}

interface Group {
  key: string;
  label: string;
  calendars: Calendar[];
}

/** One row — a synced calendar or a first-party overlay pseudo-calendar. The `hidden` set is generic
 *  over arbitrary ids, so both toggle through exactly the same path. */
function SourceRow({
  id,
  label,
  hidden,
  onToggle,
  colorOf,
  shapeIndex,
}: {
  id: string;
  label: string;
  hidden: Set<string>;
  onToggle: (calendarId: string) => void;
  colorOf: (calendarId: string) => string;
  shapeIndex?: number;
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
        <SourceDot
          color={colorOf(id)}
          shapeIndex={shapeIndex}
          className={cn(!shown && "opacity-40")}
        />
        <span className={cn("truncate", !shown && "text-ink4")}>{label}</span>
      </label>
    </li>
  );
}

export function CalendarSourceList({
  accounts,
  calendars,
  hidden,
  onToggle,
  colorOf,
  shapeOf,
  hideUnselected = false,
}: CalendarSourceListProps) {
  const shownCalendars = useMemo(
    () => (hideUnselected ? calendars.filter((c) => c.selected) : calendars),
    [calendars, hideUnselected],
  );

  const groups = useMemo<Group[]>(() => {
    const labelFor = new Map(accounts.map((a) => [a.id, a.email || a.label] as const));
    const bySource = new Map<string, Calendar[]>();
    for (const c of shownCalendars) {
      const list = bySource.get(c.source_id);
      if (list) list.push(c);
      else bySource.set(c.source_id, [c]);
    }
    return [...bySource.entries()].map(([sourceId, cals]) => ({
      key: sourceId,
      label: labelFor.get(sourceId) ?? "(calendar)",
      calendars: cals,
    }));
  }, [accounts, shownCalendars]);

  return (
    <>
      {/* The first-party overlays — pseudo-calendars you can show/hide like any synced one. Shown even
          with no calendars connected, since milestones and pinboard entries exist independently. */}
      <div className="mb-1">
        <p className="truncate px-2 pb-0.5 pt-1 font-mono text-[0.625rem] uppercase tracking-wide text-ink4">
          Personal Manager
        </p>
        <ul>
          <SourceRow
            id={MILESTONE_CALENDAR_ID}
            label="Milestones"
            hidden={hidden}
            onToggle={onToggle}
            colorOf={colorOf}
          />
          <SourceRow
            id={PINBOARD_CALENDAR_ID}
            label="Pinboard"
            hidden={hidden}
            onToggle={onToggle}
            colorOf={colorOf}
          />
        </ul>
      </div>
      {shownCalendars.length === 0 ? (
        <p className="px-2 py-2 text-xs text-ink4">No calendars connected.</p>
      ) : (
        groups.map((g) => (
          <div key={g.key} className="mb-1 last:mb-0">
            <p className="truncate px-2 pb-0.5 pt-1 font-mono text-[0.625rem] uppercase tracking-wide text-ink4">
              {g.label}
            </p>
            <ul>
              {g.calendars.map((c) => (
                <SourceRow
                  key={c.id}
                  id={c.id}
                  label={c.name}
                  hidden={hidden}
                  onToggle={onToggle}
                  colorOf={colorOf}
                  shapeIndex={shapeOf?.(c.id)}
                />
              ))}
            </ul>
          </div>
        ))
      )}
    </>
  );
}
