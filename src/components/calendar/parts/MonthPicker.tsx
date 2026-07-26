// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A month grid with ‹ › paging — the body of every "pick a day" popover in PM. Extracted from the
// calendar header's mini-calendar so the header and the app-wide `DateField` share one implementation
// rather than growing two that drift. Purely presentational: it owns which month is on screen and
// nothing else. No colour of its own; the grid itself is `MiniMonth`.

import { useEffect, useState } from "react";
import { MiniMonth } from "./MiniMonth";

interface Props {
  /** The day drawn as selected, and the month opened on. Null opens on `today`. */
  selected: Date | null;
  onPick: (d: Date) => void;
  /** Rendered under the grid — e.g. Today / Clear shortcuts. */
  footer?: React.ReactNode;
}

/** The first of the month a picker should open on for a given selection. */
function monthOf(d: Date | null): Date {
  const base = d ?? new Date();
  return new Date(base.getFullYear(), base.getMonth(), 1);
}

export function MonthPicker({ selected, onPick, footer }: Props) {
  const [view, setView] = useState(() => monthOf(selected));
  // Re-open on the selected day's month when the selection changes underneath us (typing a date in
  // the text half of a DateField should move the grid). Keyed on the ISO month so paging away with
  // ‹ › doesn't get yanked back on every unrelated re-render.
  const selectedMonthKey = selected ? `${selected.getFullYear()}-${selected.getMonth()}` : null;
  useEffect(() => {
    if (selected) setView(monthOf(selected));
    // eslint-disable-next-line react-hooks/exhaustive-deps -- keyed on the month, not the Date identity
  }, [selectedMonthKey]);

  const monthLabel = view.toLocaleDateString(undefined, { month: "long", year: "numeric" });
  return (
    <div className="w-60 p-1">
      <div className="mb-1 flex items-center justify-between">
        <button
          type="button"
          onClick={() => setView((v) => new Date(v.getFullYear(), v.getMonth() - 1, 1))}
          className="rounded-[var(--radius-sm)] px-2 py-0.5 text-ink3 hover:bg-surface"
          aria-label="Previous month"
        >
          ‹
        </button>
        <span className="font-head text-sm text-ink">{monthLabel}</span>
        <button
          type="button"
          onClick={() => setView((v) => new Date(v.getFullYear(), v.getMonth() + 1, 1))}
          className="rounded-[var(--radius-sm)] px-2 py-0.5 text-ink3 hover:bg-surface"
          aria-label="Next month"
        >
          ›
        </button>
      </div>
      <MiniMonth
        year={view.getFullYear()}
        month={view.getMonth()}
        today={new Date()}
        selected={selected}
        onSelectDay={onPick}
        showWeekdays
      />
      {footer}
    </div>
  );
}
