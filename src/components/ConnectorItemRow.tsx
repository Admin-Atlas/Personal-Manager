// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { ReactNode } from "react";
import { Button } from "./ui";

/**
 * One row of a connector's item list (a Drive/OneDrive account, or a local folder): the title + a
 * reachability dot-or-badge, an optional `detail` line (a folder's path), the `meta` line ("N indexed ·
 * synced …"), and the Sync-now / action (Disconnect / Remove) buttons. `children` renders below the row
 * for connector-specific affordances (the Google Drive "Reconnect for Sheets" nudge).
 */
export function ConnectorItemRow({
  title,
  reachable,
  badgeLabel = "unreachable",
  detail,
  meta,
  syncingThis,
  queued,
  syncDisabled,
  onSync,
  actionLabel,
  actionDisabled,
  onAction,
  children,
}: {
  title: string;
  reachable: boolean;
  /** The badge shown when not reachable ("unreachable" / "not found"). */
  badgeLabel?: string;
  detail?: ReactNode;
  meta: ReactNode;
  syncingThis: boolean;
  queued: boolean;
  syncDisabled: boolean;
  onSync: () => void;
  actionLabel: string;
  actionDisabled: boolean;
  onAction: () => void;
  children?: ReactNode;
}) {
  return (
    <div>
      <div className="flex items-center justify-between gap-2">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <span className="truncate text-sm text-ink">{title}</span>
            {reachable ? (
              <span className="h-1.5 w-1.5 shrink-0 rounded-full bg-[var(--st-quick)]" />
            ) : (
              <span className="shrink-0 text-[10px] uppercase tracking-wide text-st-due">
                {badgeLabel}
              </span>
            )}
          </div>
          {detail}
          <p className="mt-0.5 truncate text-xs text-ink4">{meta}</p>
        </div>
        <div className="flex shrink-0 items-center gap-1">
          <Button
            onClick={onSync}
            disabled={syncDisabled}
            className="px-2 py-1 text-xs disabled:opacity-40"
          >
            {syncingThis ? "Syncing…" : queued ? "Queued" : "Sync now"}
          </Button>
          <Button
            variant="tertiary"
            onClick={onAction}
            disabled={actionDisabled}
            className="px-2 py-1 text-xs hover:text-st-due"
          >
            {actionLabel}
          </Button>
        </div>
      </div>
      {children}
    </div>
  );
}
