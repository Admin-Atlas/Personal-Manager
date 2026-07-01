// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { cn } from "../../ui";

interface Props {
  /** The calendar's source colour — feature state, resolved from the categorical source palette
   *  (not a theme token), so it arrives as a prop rather than a hex literal in this component. */
  color: string;
  className?: string;
}

/** The small colour dot that identifies a calendar (source) in the dropdown, agenda rows, and legend.
 *  Reserves the accent for chrome, so a source is never confusable with "now". */
export function SourceDot({ color, className }: Props) {
  return (
    <span
      aria-hidden
      className={cn("inline-block h-2.5 w-2.5 shrink-0 rounded-full", className)}
      style={{ backgroundColor: color }}
    />
  );
}
