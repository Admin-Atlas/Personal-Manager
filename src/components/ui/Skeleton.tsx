// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { CSSProperties } from "react";
import { cn } from "./cn";

// Loading placeholder (DESIGN_TOKENS.md §7). A --surface block with a shimmer sweep
// (--surface → --border → --surface). The pm-shimmer keyframes only exist when motion is allowed
// (index.css), so under prefers-reduced-motion this rests as a static block.
export interface SkeletonProps {
  className?: string;
  style?: CSSProperties;
}

export function Skeleton({ className, style }: SkeletonProps) {
  return (
    <div
      aria-hidden
      className={cn("rounded-[var(--radius-sm)]", className)}
      style={{
        background: "linear-gradient(90deg, var(--surface), var(--border), var(--surface))",
        backgroundSize: "200% 100%",
        animation: "pm-shimmer 1.4s linear infinite",
        ...style,
      }}
    />
  );
}
