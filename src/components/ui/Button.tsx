// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Token-driven button (DESIGN_TOKENS.md §7). Three variants; the terminal System wraps the
// label in brackets and forces mono. Never style a button instance directly — use this.

import type { ComponentPropsWithRef, ReactNode } from "react";
import { useTheme } from "../../theme";
import { cn } from "./cn";

export type ButtonVariant = "primary" | "secondary" | "tertiary";

const VARIANT: Record<ButtonVariant, string> = {
  primary:
    "bg-accent text-accent-ink font-semibold hover:brightness-105 active:brightness-95 disabled:bg-surface disabled:text-faint",
  secondary:
    "bg-transparent text-ink2 border border-border2 hover:bg-surface disabled:text-faint",
  tertiary: "bg-transparent text-ink4 hover:text-ink2 disabled:text-faint",
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
        "inline-flex items-center justify-center gap-1.5 rounded-[var(--radius-sm)] px-3 py-1.5 text-sm transition disabled:cursor-not-allowed",
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
