// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Sizes a 12-month Year-view grid to its measured container instead of a fixed compact size — shared
// by the Slate/Editorial YearView and TerminalYearTable so both scale identically with the window.

import { useEffect, useRef, useState, type RefObject } from "react";

// Layout budget for fitting fixed-6-week-row months into the measured container: a title row, a
// weekday-label row, gaps between months, and a min/max clamp on the resulting day-cell so text never
// vanishes or balloons absurdly on extreme window sizes.
const TITLE_PX = 24;
const WEEKDAY_PX = 18;
export const ROW_GAP_PX = 16;
export const COL_GAP_PX = 24;
const MIN_CELL_PX = 20;
const MAX_CELL_PX = 48;

export interface YearGridLayout {
  containerRef: RefObject<HTMLDivElement | null>;
  cols: number;
  rows: number;
  cellPx: number;
}

/** Picks a column count from the measured width, then a day-cell px that fills the resulting rows
 *  (assuming the worst case of a 6-week-row month), clamped to a readable range. */
export function useYearGridLayout(monthCount: number): YearGridLayout {
  const containerRef = useRef<HTMLDivElement>(null);
  const [size, setSize] = useState({ w: 0, h: 0 });

  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;
    const ro = new ResizeObserver((entries) => {
      const r = entries[0]?.contentRect;
      if (r) setSize({ w: r.width, h: r.height });
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const cols = size.w >= 1400 ? 4 : size.w >= 1000 ? 3 : size.w >= 620 ? 2 : 1;
  const rows = Math.ceil(monthCount / cols);
  const rowBudget = size.h > 0 ? (size.h - ROW_GAP_PX * (rows - 1)) / rows : 0;
  const cellPx =
    rowBudget > 0
      ? Math.max(
          MIN_CELL_PX,
          Math.min(MAX_CELL_PX, Math.floor((rowBudget - TITLE_PX - WEEKDAY_PX) / 6)),
        )
      : MIN_CELL_PX;

  return { containerRef, cols, rows, cellPx };
}
