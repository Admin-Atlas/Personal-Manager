// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Token-driven text input (DESIGN_TOKENS.md §7). Terminal flavour: mono + accent caret.
// (The ❯ prompt adornment is applied at call sites that want it, e.g. the composer/palette.)

import type { InputHTMLAttributes } from "react";
import { useTheme } from "../../theme";
import { cn } from "./cn";

export function Input({ className, ...rest }: InputHTMLAttributes<HTMLInputElement>) {
  const { system } = useTheme();
  return (
    <input
      className={cn(
        "w-full rounded-[var(--radius-sm)] border border-border2 bg-surface px-3 py-2 text-sm text-ink2 outline-none transition placeholder:text-ink4 focus:border-accent",
        system === "terminal" && "font-mono caret-[var(--accent-text)]",
        className,
      )}
      {...rest}
    />
  );
}
