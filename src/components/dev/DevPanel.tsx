// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The shared shell for every Developer-mode surface (issue #78): a labelled, bordered card.
// Cross-cutting diagnostics live in DevView; in-context ones (PR 2) wrap their body in this same
// shell behind `useDevMode()`. Read-only — a DevPanel only ever displays state, never mutates it.

import type { ReactNode } from "react";
import { Card, SectionLabel } from "../ui";

export interface DevPanelProps {
  title: string;
  subtitle?: string;
  /** Optional right-aligned controls (e.g. a refresh button or a table picker). */
  actions?: ReactNode;
  helpId?: string;
  children: ReactNode;
  className?: string;
}

export function DevPanel({ title, subtitle, actions, helpId, children, className }: DevPanelProps) {
  return (
    <Card className={`p-4 ${className ?? ""}`}>
      <div className="flex items-start justify-between gap-3" data-help={helpId}>
        <div>
          <SectionLabel as="h3">{title}</SectionLabel>
          {subtitle && <p className="mt-0.5 text-xs text-ink4">{subtitle}</p>}
        </div>
        {actions && <div className="shrink-0">{actions}</div>}
      </div>
      <div className="mt-3">{children}</div>
    </Card>
  );
}
