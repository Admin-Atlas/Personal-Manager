// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

/** The Pinboard model (spec §4 — a bounded planning board). Persisted to localStorage as
 *  presentation/arrangement state, like the theme — not backend data (it isn't a source of
 *  truth and needs no encryption). All geometry is in **grid cells**, not pixels, so the
 *  board renders crisply at any cell size. */

export type WidgetKind = "note" | "timeline" | "folder";

/** A widget's position and size, in grid cells (top-left origin). */
export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** One dated entry on a timeline widget. `date` is an ISO date (YYYY-MM-DD); rendered DD-MM-YYYY. */
export interface TimelineItem {
  id: string;
  date: string;
  label: string;
}

export interface Widget {
  id: string;
  kind: WidgetKind;
  rect: Rect;
  /** A design-token name (`st-quick` etc.) used to tint the widget; undefined → neutral. */
  color?: string;
  // note
  text?: string;
  /** Set once a note has been ingested as a document (ISO time). The document is keyed on the
   *  widget id (`note:<id>`) and lives independently — deleting the note never removes it. */
  ingestedAt?: string;
  /** A cheap client-side hash of the note text at last ingest, so the UI can offer "re-ingest
   *  edits" when the text has since changed. Not the backend content hash. */
  ingestedHash?: string;
  /** A short user label shown (and edited) in the widget's top bar. Originally timeline-only;
   *  now the shared title for every kind (note / timeline / folder). Empty → the header shows a
   *  kind placeholder. Optional and additive, so old boards parse unchanged. */
  title?: string;
  // timeline
  items?: TimelineItem[];
  /** When set, a timeline widget is *bound* to this project: it shows and edits that project's
   *  real milestones (backend `project_milestones`) instead of the freeform `items` above, so
   *  changes flow to the daily brief and the project's Focus/sidebar. Unset → freeform (default).
   *  Optional and additive, so old boards parse unchanged and `BOARD_VERSION` stays 1. */
  project?: string;
  /** Freeform timelines only: whether this widget's dated entries also appear on the Calendar tab
   *  (the pinboard overlay). Unset → shown, so entries land on the calendar by default. A
   *  project-bound timeline ignores it — those entries are real milestones, already drawn by the
   *  milestone overlay. Optional and additive, so old boards parse unchanged and `BOARD_VERSION`
   *  stays 1. */
  showOnCalendar?: boolean;
  /** How a timeline widget lays its entries out: `"list"` (stacked rows) or `"row"` (a
   *  horizontal date-ordered track). Unset falls back to each kind's historical default —
   *  freeform → list, project-bound → row — so old boards look identical. Optional and
   *  additive, so `BOARD_VERSION` stays 1. */
  view?: "list" | "row";
  // folder (kind === "folder")
  /** The widgets contained by a folder. Notes and timelines only — folders never nest (enforced
   *  on drop and on load). A collapsed folder shows the child count; expanding reveals the cards.
   *  Additive, so old boards (which have none) parse unchanged and `BOARD_VERSION` stays 1. */
  children?: Widget[];
  /** How an opened folder is presented, persisted per-folder. Default `"inline"` (an in-place
   *  panel); `"overlay"` opens a centred modal. Whether the folder is *currently* expanded is
   *  transient UI state and is deliberately NOT stored here. */
  expandMode?: "inline" | "overlay";
}

export interface Board {
  /** Schema version, so a future shape change can migrate or discard old localStorage state. */
  version: number;
  widgets: Widget[];
}

export const BOARD_VERSION = 1;

/** The settings-table key the board persists under (must match the backend `set_pref` allowlist).
 *  Shared so read-only consumers (the calendar's pinboard overlay) can't drift from the writer. */
export const PINBOARD_PREF_KEY = "pinboard";

export const EMPTY_BOARD: Board = { version: BOARD_VERSION, widgets: [] };
