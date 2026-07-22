// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Sidebar navigation item (DESIGN_TOKENS.md §7). Active treatment diverges per System:
// editorial/terminal get an accent left-border, slate gets a surface fill. Inactive rests at
// --ink3 and lifts to --ink2 on hover.

import type { ReactNode } from "react";
import { useTheme } from "../../theme";
import { cn } from "./cn";

export interface NavItemProps {
  active?: boolean;
  onClick?: () => void;
  children: ReactNode;
  /** Optional leading slot — e.g. a tab icon. Inherits the item's text colour (active/inactive). */
  leading?: ReactNode;
  trailing?: ReactNode;
  helpId?: string;
  className?: string;
}

export function NavItem({
  active,
  onClick,
  children,
  leading,
  trailing,
  helpId,
  className,
}: NavItemProps) {
  const { system } = useTheme();
  const leftBorder = system === "editorial" || system === "terminal";
  return (
    <button
      type="button"
      data-help={helpId}
      onClick={onClick}
      className={cn(
        "flex w-full items-center justify-between gap-2 rounded-[var(--radius-sm)] px-3 py-1.5 text-left text-sm transition",
        leftBorder && "border-l-2 border-transparent",
        active
          ? cn("bg-surface text-ink", leftBorder && "border-accent")
          : "text-ink3 hover:bg-surface hover:text-ink2",
        system === "terminal" && "font-mono",
        className,
      )}
    >
      {leading != null && <span className="shrink-0">{leading}</span>}
      <span className="min-w-0 flex-1 truncate">{children}</span>
      {trailing != null && <span className="shrink-0">{trailing}</span>}
    </button>
  );
}
