// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { ComponentPropsWithRef } from "react";
import { useTheme } from "../../theme";
import { cn } from "./cn";

// Token-driven multiline input — same surface/border/focus treatment as Input. Uses
// ComponentPropsWithRef so callers (e.g. Composer's focus ref) can forward a ref.
export function Textarea({ className, ...rest }: ComponentPropsWithRef<"textarea">) {
  const { system } = useTheme();
  return (
    <textarea
      className={cn(
        "w-full resize-none rounded-[var(--radius-sm)] border border-border2 bg-surface px-3 py-2 text-sm text-ink2 outline-none transition placeholder:text-ink4 focus:border-accent",
        system === "terminal" && "font-mono caret-[var(--accent-text)]",
        className,
      )}
      {...rest}
    />
  );
}
