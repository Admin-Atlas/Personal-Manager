// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A filterable list of IANA time zones: type a continent, country, city or code, pick one. Lifted out
// of the time grid's add-a-zone popover (ZoneGutter) so the event editor's zone picker is the same
// control. Token-driven; the caller supplies the popover or panel around it.

import { useState } from "react";
import { allZoneOptions } from "../../lib/zoneLabel";

interface Props {
  /** Zones not to offer (ones already chosen). */
  exclude?: readonly string[];
  onPick: (zone: string) => void;
  autoFocus?: boolean;
}

/** How many matches are listed at once; the filter narrows the rest. */
const MAX_MATCHES = 40;

export function ZoneSearchList({ exclude = [], onPick, autoFocus }: Props) {
  const [filter, setFilter] = useState("");
  const q = filter.trim().toLowerCase();
  const matches = allZoneOptions()
    .filter((o) => !exclude.includes(o.id) && o.search.includes(q))
    .slice(0, MAX_MATCHES);

  return (
    <div className="space-y-2">
      <input
        value={filter}
        onChange={(e) => setFilter(e.target.value)}
        placeholder="Search continent, country, city or code…"
        aria-label="Filter timezones"
        autoFocus={autoFocus}
        className="w-full rounded-[var(--radius-sm)] border border-border2 bg-surface px-2 py-1 text-xs text-ink2 focus:border-accent focus:outline-none"
      />
      <ul className="max-h-56 overflow-auto">
        {matches.map((o) => (
          <li key={o.id}>
            <button
              type="button"
              onClick={() => {
                onPick(o.id);
                setFilter("");
              }}
              title={o.id}
              className="flex w-full items-center justify-between gap-2 rounded-[var(--radius-sm)] px-2 py-1 text-left text-xs text-ink3 hover:bg-surface hover:text-ink"
            >
              <span className="truncate">{o.label}</span>
              <span className="shrink-0 font-mono text-[0.625rem] text-ink4">{o.code}</span>
            </button>
          </li>
        ))}
        {matches.length === 0 && q && (
          <li className="px-2 py-1 text-[0.6875rem] text-ink4">No match.</li>
        )}
      </ul>
    </div>
  );
}
