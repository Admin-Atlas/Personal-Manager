// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

/** The Pinboard model (spec §4 — a bounded planning board). Persisted to localStorage as
 *  presentation/arrangement state, like the theme — not backend data (it isn't a source of
 *  truth and needs no encryption). All geometry is in **grid cells**, not pixels, so the
 *  board renders crisply at any cell size. */

export type WidgetKind = "note" | "timeline";

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
  // timeline
  title?: string;
  items?: TimelineItem[];
}

export interface Board {
  /** Schema version, so a future shape change can migrate or discard old localStorage state. */
  version: number;
  widgets: Widget[];
}

export const BOARD_VERSION = 1;

export const EMPTY_BOARD: Board = { version: BOARD_VERSION, widgets: [] };
