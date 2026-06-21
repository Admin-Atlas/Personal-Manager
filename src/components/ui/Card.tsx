// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { HTMLAttributes } from "react";
import { cn } from "./cn";

// A raised surface. `variant="card"` is the bordered box (slate/terminal); `variant="rule"` is
// the editorial "set like a page" treatment — a top hairline + breathing room instead of a box.
// Call sites pick per-System where the design diverges; most just use "card".
export interface CardProps extends HTMLAttributes<HTMLDivElement> {
  variant?: "card" | "rule";
}

export function Card({ variant = "card", className, children, ...rest }: CardProps) {
  return (
    <div
      className={cn(
        variant === "card"
          ? "rounded-[var(--radius)] border border-border bg-surface"
          : "border-t border-border pt-3",
        className,
      )}
      {...rest}
    >
      {children}
    </div>
  );
}
