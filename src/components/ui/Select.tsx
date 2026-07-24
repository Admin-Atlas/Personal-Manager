// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { ComponentPropsWithRef } from "react";
import { useTheme } from "../../theme";
import { cn } from "./cn";

interface Props extends ComponentPropsWithRef<"select"> {
  /** Compact sizing for dense toolbars. Height comes from symmetric padding + the font's own
   *  line-height — never a fixed `h-*` — so the selected text stays vertically centred on every
   *  engine. (WebKitGTK on Linux anchors native-<select> text lower than Blink, so a fixed height
   *  with zeroed padding clipped the descenders there; padding-driven sizing renders identically.) */
  compact?: boolean;
}

// Token-driven wrapper over the native <select>. color-scheme (set by applyTheme) makes the
// native dropdown follow light/dark automatically.
export function Select({ className, children, compact = false, ...rest }: Props) {
  const { system } = useTheme();
  // Swap the size classes, never layer them: cn() is a plain joiner, so emitting both py-1.5 and py-1
  // would leave the winner to stylesheet order — the ambiguity that let the clip through.
  const sizing = compact ? "px-2 py-1 text-xs" : "px-2 py-1.5 text-sm";
  return (
    <select
      className={cn(
        "rounded-[var(--radius-sm)] border border-border2 bg-surface text-ink2 outline-none transition focus:border-accent",
        sizing,
        system === "terminal" && "font-mono",
        className,
      )}
      {...rest}
    >
      {children}
    </select>
  );
}
