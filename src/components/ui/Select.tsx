// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { SelectHTMLAttributes, ReactNode } from "react";
import { useTheme } from "../../theme";
import { cn } from "./cn";

// Token-driven wrapper over the native <select>. color-scheme (set by applyTheme) makes the
// native dropdown follow light/dark automatically.
export function Select({
  className,
  children,
  ...rest
}: SelectHTMLAttributes<HTMLSelectElement> & { children?: ReactNode }) {
  const { system } = useTheme();
  return (
    <select
      className={cn(
        "rounded-[var(--radius-sm)] border border-border2 bg-surface px-2 py-1.5 text-sm text-ink2 outline-none transition focus:border-accent",
        system === "terminal" && "font-mono",
        className,
      )}
      {...rest}
    >
      {children}
    </select>
  );
}
