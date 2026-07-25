// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Token-driven button (DESIGN_TOKENS.md §7). Three variants; the terminal System wraps the
// label in brackets and forces mono. Never style a button instance directly — use this.

import type { ComponentPropsWithRef, ReactNode } from "react";
import { useTheme } from "../../theme";
import { cn } from "./cn";

export type ButtonVariant = "primary" | "secondary" | "tertiary";

// Hover/active are gated to :enabled so a disabled button never reacts to the pointer — combined
// with the base disabled:opacity-40 it reads as unmistakably inert, not merely dimmed.
const VARIANT: Record<ButtonVariant, string> = {
  primary:
    "bg-accent text-accent-ink font-semibold enabled:hover:brightness-105 enabled:active:brightness-95 disabled:bg-surface disabled:text-faint",
  secondary:
    "bg-transparent text-ink2 border border-border2 enabled:hover:bg-surface disabled:text-faint",
  tertiary: "bg-transparent text-ink4 enabled:hover:text-ink2 disabled:text-faint",
};

export interface ButtonProps extends ComponentPropsWithRef<"button"> {
  variant?: ButtonVariant;
  children?: ReactNode;
}

export function Button({ variant = "secondary", className, children, ...rest }: ButtonProps) {
  const { system } = useTheme();
  const terminal = system === "terminal";
  return (
    <button
      className={cn(
        "inline-flex min-h-[var(--tap-min,24px)] items-center justify-center gap-1.5 rounded-[var(--radius-sm)] px-3 py-1.5 text-sm transition disabled:cursor-not-allowed disabled:opacity-40",
        terminal && "font-mono",
        VARIANT[variant],
        className,
      )}
      {...rest}
    >
      {terminal ? <>[&nbsp;{children}&nbsp;]</> : children}
    </button>
  );
}
