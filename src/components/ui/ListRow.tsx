// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { ReactNode } from "react";
import { cn } from "./cn";

// A divider-separated row: head-font title + optional mono meta + optional trailing slot. Backs
// the Focus rows, Review rows, project file lists, and the Documents list. Divider uses --rule
// (the faint row divider), title --ink/--head, meta --ink4/--mono.
export interface ListRowProps {
  title: ReactNode;
  meta?: ReactNode;
  trailing?: ReactNode;
  active?: boolean;
  onClick?: () => void;
  className?: string;
  helpId?: string;
}

export function ListRow({
  title,
  meta,
  trailing,
  active,
  onClick,
  className,
  helpId,
}: ListRowProps) {
  const interactive = typeof onClick === "function";
  return (
    <div
      data-help={helpId}
      onClick={onClick}
      className={cn(
        "flex items-center justify-between gap-3 border-b border-rule px-3 py-2.5",
        interactive && "cursor-pointer transition hover:bg-surface",
        active && "bg-surface",
        className,
      )}
    >
      <div className="min-w-0">
        <div className="truncate font-head text-sm text-ink">{title}</div>
        {meta != null && <div className="mt-0.5 truncate font-mono text-xs text-ink4">{meta}</div>}
      </div>
      {trailing != null && <div className="shrink-0">{trailing}</div>}
    </div>
  );
}
