// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Add/remove up to MAX_EXTRA_ZONES extra timezones shown in the Day/Week gutter beside the local
// time. A Popover with the current zones as removable chips and a filtered list of every IANA zone
// (a ~400-entry native <select> is unusable, so a substring filter is friendlier). Token-driven.

import { useState } from "react";
import { allTimeZones } from "../../theme";
import { MAX_EXTRA_ZONES } from "../../lib/calendarPrefs";
import { Popover } from "./Popover";

/** Short, friendly display for a zone: last path segment with underscores as spaces (e.g. "New York"). */
function zoneShort(tz: string): string {
  return (tz.split("/").pop() ?? tz).replace(/_/g, " ");
}

interface Props {
  zones: string[];
  onChange: (zones: string[]) => void;
}

export function ZonePicker({ zones, onChange }: Props) {
  const [filter, setFilter] = useState("");
  const atCap = zones.length >= MAX_EXTRA_ZONES;
  const q = filter.trim().toLowerCase();
  const matches = atCap
    ? []
    : allTimeZones()
        .filter((z) => !zones.includes(z) && z.toLowerCase().includes(q))
        .slice(0, 40);

  return (
    <Popover
      align="right"
      ariaLabel="Extra timezones"
      panelClassName="min-w-[16rem] p-2"
      trigger={({ toggle, open }) => (
        <button
          type="button"
          onClick={toggle}
          aria-pressed={open}
          title="Show extra timezones in the gutter"
          className="rounded-[var(--radius-sm)] border border-border2 px-2 py-1 text-xs text-ink3 transition hover:text-ink"
        >
          Zones{zones.length ? ` (${zones.length})` : ""}
        </button>
      )}
    >
      <div className="space-y-2">
        {zones.length > 0 ? (
          <ul className="space-y-1">
            {zones.map((z) => (
              <li key={z} className="flex items-center justify-between gap-2 text-xs text-ink2">
                <span className="truncate" title={z}>
                  {zoneShort(z)}
                </span>
                <button
                  type="button"
                  onClick={() => onChange(zones.filter((x) => x !== z))}
                  aria-label={`Remove ${z}`}
                  className="shrink-0 rounded-[var(--radius-sm)] px-1 text-ink4 hover:text-st-due"
                >
                  ✕
                </button>
              </li>
            ))}
          </ul>
        ) : (
          <p className="text-[11px] text-ink4">
            Show up to {MAX_EXTRA_ZONES} more zones in the day/week gutter.
          </p>
        )}
        {atCap ? (
          <p className="text-[11px] text-ink4">Maximum of {MAX_EXTRA_ZONES} extra zones.</p>
        ) : (
          <>
            <input
              value={filter}
              onChange={(e) => setFilter(e.target.value)}
              placeholder="Add a timezone…"
              aria-label="Filter timezones"
              className="w-full rounded-[var(--radius-sm)] border border-border2 bg-surface px-2 py-1 text-xs text-ink2 focus:border-accent focus:outline-none"
            />
            <ul className="max-h-48 overflow-auto">
              {matches.map((z) => (
                <li key={z}>
                  <button
                    type="button"
                    onClick={() => {
                      onChange([...zones, z]);
                      setFilter("");
                    }}
                    title={z}
                    className="block w-full truncate rounded-[var(--radius-sm)] px-2 py-1 text-left text-xs text-ink3 hover:bg-surface hover:text-ink"
                  >
                    {z.replace(/_/g, " ")}
                  </button>
                </li>
              ))}
              {matches.length === 0 && q && (
                <li className="px-2 py-1 text-[11px] text-ink4">No match.</li>
              )}
            </ul>
          </>
        )}
      </div>
    </Popover>
  );
}
