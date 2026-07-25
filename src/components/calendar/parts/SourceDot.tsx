// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useTheme } from "../../../theme";
import { cn } from "../../ui";

// Shape redundancy for the colour-blind axis: when it's on, each source's dot also takes a distinct
// SHAPE (tracking its slot), so calendars are told apart without relying on hue — the legend
// (CalendarsDropdown) shows the same shape beside each name, which is what makes the grid dots
// readable. Index 0 is the plain circle (so `shapeIndex` 0 and the axis-off case render identically);
// the rest are clip-path polygons picked to stay legible at ~8–10px. Beyond the set they repeat by
// modulo, but the colour still differs — it's the shape×colour pair that separates sources.
const SHAPES: readonly (string | null)[] = [
  null, // circle
  "polygon(50% 0, 100% 50%, 50% 100%, 0 50%)", // diamond
  "polygon(50% 0, 100% 100%, 0 100%)", // triangle up
  "polygon(0 0, 100% 0, 100% 100%, 0 100%)", // square (sharp corners)
  "polygon(0 0, 100% 0, 50% 100%)", // triangle down
  "polygon(25% 5%, 75% 5%, 100% 50%, 75% 95%, 25% 95%, 0 50%)", // hexagon
];

interface Props {
  /** The calendar's source colour — feature state, resolved from the categorical source palette
   *  (not a theme token), so it arrives as a prop rather than a hex literal in this component. */
  color: string;
  /** The source's slot (from sourceShapeIndex), used to pick a redundant shape when the colour-blind
   *  axis is on. Omit (overlays, unknowns) to keep the plain circle. */
  shapeIndex?: number;
  className?: string;
}

/** The small colour dot that identifies a calendar (source) in the dropdown, agenda rows, and legend.
 *  Reserves the accent for chrome, so a source is never confusable with "now". Under the colour-blind
 *  axis it also carries a per-source shape (see {@link SHAPES}). */
export function SourceDot({ color, shapeIndex, className }: Props) {
  const { colorblind } = useTheme();
  const clip = colorblind && shapeIndex != null ? SHAPES[shapeIndex % SHAPES.length] : null;
  return (
    <span
      aria-hidden
      className={cn("inline-block h-2.5 w-2.5 shrink-0", clip == null && "rounded-full", className)}
      style={{ backgroundColor: color, clipPath: clip ?? undefined }}
    />
  );
}
