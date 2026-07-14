// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Lifts the Pinboard's freeform timeline entries onto the Calendar tab (the "pm:pinboard" overlay).
// Pure and separate from the view so the awkward cases — folders, project-bound timelines, opted-out
// widgets, a corrupt stored board — are unit-testable rather than only reachable through the UI.

import { BOARD_VERSION, type Board, type Widget } from "./types";

/** One dated freeform timeline entry, flattened out of the board for the calendar overlay. */
export interface PinboardEntry {
  widgetId: string;
  itemId: string;
  /** ISO date (YYYY-MM-DD) — the day the entry sits on. */
  date: string;
  label: string;
}

/** Shown when an entry has a date but no text yet, so a dated row never renders as an empty chip.
 *  Matches the fallback `linkProject` writes when the same entry becomes a real milestone
 *  (PinboardView), so a blank row doesn't get renamed the moment its timeline is linked. */
const UNTITLED = "deadline";

/** Every dated entry of every freeform timeline on the board, in board order.
 *
 *  Skipped deliberately:
 *  - a timeline **bound to a project** (`project` set) — its entries are real `project_milestones`
 *    already drawn by the milestone overlay, so including them here would double-draw;
 *  - a timeline the user opted out of (`showOnCalendar === false`) — unset means shown, so entries
 *    land on the calendar by default;
 *  - entries with no date — they have no day to sit on.
 *
 *  Folders never nest, so a depth-1 walk reaches every widget. The board is stored JSON, so this
 *  parses defensively: anything malformed yields no entries rather than throwing into the calendar.
 *  Version mismatch returns nothing, matching the board's own parse (which discards it wholesale). */
export function pinboardEntries(raw: string | null): PinboardEntry[] {
  if (!raw) return [];
  let parsed: Board;
  try {
    parsed = JSON.parse(raw) as Board;
  } catch {
    return [];
  }
  if (!parsed || parsed.version !== BOARD_VERSION || !Array.isArray(parsed.widgets)) return [];

  const out: PinboardEntry[] = [];
  const visit = (w: Widget | undefined) => {
    if (!w || typeof w !== "object") return;
    if (w.kind === "folder") {
      for (const child of w.children ?? []) visit(child);
      return;
    }
    if (w.kind !== "timeline") return;
    if (w.project) return;
    if (w.showOnCalendar === false) return;
    if (typeof w.id !== "string") return;
    for (const it of w.items ?? []) {
      if (!it || typeof it.id !== "string" || typeof it.date !== "string") continue;
      const date = it.date.slice(0, 10);
      if (!date) continue;
      out.push({
        widgetId: w.id,
        itemId: it.id,
        date,
        label: (typeof it.label === "string" ? it.label.trim() : "") || UNTITLED,
      });
    }
  };
  for (const w of parsed.widgets) visit(w);
  return out;
}
