// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Day/Week grid's top-left gutter corner: the extra-timezone column headers (each removable on
// hover) plus the add control that opens a filtered IANA-zone picker. It sits where the vertical time
// axis meets the horizontal date header — the space-saving home for zone management, replacing the old
// header "Zones" button. The local column stays nearest the grid and is never removable. Token-driven.

import { useState } from "react";
import { deviceTimeZone } from "../../theme";
import { MAX_EXTRA_ZONES } from "../../lib/calendarPrefs";
import { allZoneOptions } from "../../lib/zoneLabel";
import { cn } from "../ui";
import { Popover } from "./Popover";

/** Short, friendly display for a zone: last path segment with underscores as spaces (e.g. "New York"). */
function zoneShort(tz: string): string {
  return (tz.split("/").pop() ?? tz).replace(/_/g, " ");
}

interface Props {
  zones: string[];
  onChange: (zones: string[]) => void;
  /** Column widths (px), matching the body gutter so each header sits over its own hour-label column. */
  zoneCol: number;
  localCol: number;
}

/** The corner add/remove strip. Width mirrors the body gutter (local + one column per extra zone). */
export function ZoneGutter({ zones, onChange, zoneCol, localCol }: Props) {
  const atCap = zones.length >= MAX_EXTRA_ZONES;
  const width = localCol + zones.length * zoneCol;

  return (
    <div className="flex shrink-0" style={{ width: `${width}px` }}>
      {zones.map((z) => (
        <div
          key={z}
          className="group relative flex items-end justify-end border-l border-rule px-1 pb-0.5 font-mono text-[9px] uppercase tracking-tight text-ink4"
          style={{ width: `${zoneCol}px` }}
          title={z}
        >
          <span className="truncate">{zoneShort(z)}</span>
          <button
            type="button"
            onClick={() => onChange(zones.filter((x) => x !== z))}
            aria-label={`Remove ${z}`}
            title={`Remove ${zoneShort(z)}`}
            className="absolute right-0 top-0 rounded-[var(--radius-sm)] px-0.5 leading-none text-ink4 opacity-0 transition hover:text-st-due focus-visible:opacity-100 group-hover:opacity-100"
          >
            ✕
          </button>
        </div>
      ))}
      <div
        className="flex items-end justify-end gap-1 px-1 pb-0.5"
        style={{ width: `${localCol}px` }}
      >
        {!atCap && <AddZone zones={zones} onChange={onChange} compact={zones.length > 0} />}
        {zones.length > 0 && (
          <span
            className="truncate font-mono text-[9px] uppercase tracking-tight text-ink4"
            title={deviceTimeZone()}
          >
            {zoneShort(deviceTimeZone())}
          </span>
        )}
      </div>
    </div>
  );
}

/** The "+ Add" / "+" trigger and its zone-picker popover. Add-only — removal lives on the headers. */
function AddZone({
  zones,
  onChange,
  compact,
}: {
  zones: string[];
  onChange: (zones: string[]) => void;
  compact: boolean;
}) {
  const [filter, setFilter] = useState("");
  const q = filter.trim().toLowerCase();
  const matches = allZoneOptions()
    .filter((o) => !zones.includes(o.id) && o.search.includes(q))
    .slice(0, 40);

  return (
    <Popover
      align="left"
      ariaLabel="Add a timezone"
      panelClassName="min-w-[19rem] p-2"
      trigger={({ toggle, open }) => (
        <button
          type="button"
          onClick={toggle}
          aria-pressed={open}
          aria-label="Add a timezone"
          title="Add a timezone"
          className={cn(
            "shrink-0 rounded-[var(--radius-sm)] font-mono text-[10px] uppercase leading-none tracking-tight text-ink4 transition hover:text-ink",
            compact ? "px-0.5 py-0.5" : "border border-border2 px-1 py-0.5",
          )}
        >
          {compact ? "+" : "+ Add"}
        </button>
      )}
    >
      {({ close }) => (
        <div className="space-y-2">
          <input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            placeholder="Search continent, country, city or code…"
            aria-label="Filter timezones"
            autoFocus
            className="w-full rounded-[var(--radius-sm)] border border-border2 bg-surface px-2 py-1 text-xs text-ink2 focus:border-accent focus:outline-none"
          />
          <ul className="max-h-56 overflow-auto">
            {matches.map((o) => (
              <li key={o.id}>
                <button
                  type="button"
                  onClick={() => {
                    onChange([...zones, o.id]);
                    setFilter("");
                    close();
                  }}
                  title={o.id}
                  className="flex w-full items-center justify-between gap-2 rounded-[var(--radius-sm)] px-2 py-1 text-left text-xs text-ink3 hover:bg-surface hover:text-ink"
                >
                  <span className="truncate">{o.label}</span>
                  <span className="shrink-0 font-mono text-[10px] text-ink4">{o.code}</span>
                </button>
              </li>
            ))}
            {matches.length === 0 && q && (
              <li className="px-2 py-1 text-[11px] text-ink4">No match.</li>
            )}
          </ul>
        </div>
      )}
    </Popover>
  );
}
