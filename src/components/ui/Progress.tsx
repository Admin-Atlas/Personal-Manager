// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// A token-driven progress bar that fills as work completes. Pass a 0–1 `value` for a
// determinate fill (ingest/embed/convert progress); omit it (or pass null) for an
// indeterminate accent sweep while work runs with no known total. The indeterminate
// sweep reuses the `pm-shimmer` keyframe (index.css), which only exists when motion is
// allowed — so under prefers-reduced-motion it rests as a static accent gradient.

import type { CSSProperties } from "react";
import { cn } from "./cn";

export interface ProgressProps {
  /** 0–1 determinate fraction, or null/undefined for an indeterminate sweep. */
  value?: number | null;
  className?: string;
  /** Accessible label for the progressbar. */
  label?: string;
  /** Spoken form of the current value (e.g. "3 of 10"), so a screen reader announces the count
   *  rather than a bare percentage. Ignored while indeterminate. */
  valueText?: string;
}

const SWEEP: CSSProperties = {
  background: "linear-gradient(90deg, var(--accent-soft), var(--accent), var(--accent-soft))",
  backgroundSize: "200% 100%",
  animation: "pm-shimmer 1.4s linear infinite",
};

export function Progress({ value, className, label = "Loading", valueText }: ProgressProps) {
  const determinate = typeof value === "number";
  const pct = determinate ? Math.max(0, Math.min(1, value as number)) * 100 : undefined;
  return (
    <div
      role="progressbar"
      aria-label={label}
      aria-valuemin={determinate ? 0 : undefined}
      aria-valuemax={determinate ? 100 : undefined}
      aria-valuenow={determinate ? Math.round(pct as number) : undefined}
      aria-valuetext={determinate ? valueText : undefined}
      className={cn("h-1 w-full overflow-hidden rounded-[var(--radius-sm)] bg-border", className)}
    >
      {determinate ? (
        <div
          className="h-full rounded-[var(--radius-sm)] bg-accent transition-[width] duration-300 ease-out"
          style={{ width: `${pct}%` }}
        />
      ) : (
        <div className="h-full w-full" style={SWEEP} />
      )}
    </div>
  );
}
