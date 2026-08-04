// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { ReactNode } from "react";
import { Button, Popover } from "./ui";

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
  onReindex,
  reindexDisabled = false,
  reindexBlockedReason,
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
  /** Re-index this item from scratch (forget the delta cursor and re-enumerate). Omitted by
   *  connectors that have no cursor to discard, in which case no split control is rendered at all —
   *  an empty menu beside every row would be worse than no menu. */
  onReindex?: () => void;
  /** Whether re-indexing is unavailable right now (a sync is in flight). */
  reindexDisabled?: boolean;
  /** Why it is unavailable — shown INSIDE the open menu rather than as a tooltip on a dead control,
   *  because a disabled item that explains nothing reads as a bug (Settings info/control doctrine). */
  reindexBlockedReason?: string;
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
              <span className="shrink-0 text-[0.625rem] uppercase tracking-wide text-st-due">
                {badgeLabel}
              </span>
            )}
          </div>
          {detail}
          <p className="mt-0.5 truncate text-xs text-ink4">{meta}</p>
        </div>
        <div className="flex shrink-0 items-center gap-1">
          {/* A split control, not two peer buttons: "Sync now" is the everyday action — one request
              that usually returns an empty page — while re-indexing costs a full listing of the
              account. Keeping the cheap one a single click and folding the expensive one behind a
              caret is the whole point; side by side they would read as equally routine. */}
          <div className="flex items-center">
            <Button
              size="sm"
              onClick={onSync}
              disabled={syncDisabled}
              className={onReindex ? "rounded-r-none" : undefined}
            >
              {syncingThis ? "Syncing…" : queued ? "Queued" : "Sync now"}
            </Button>
            {onReindex && (
              <Popover
                align="right"
                escapeClipping
                ariaLabel="Sync options"
                panelClassName="w-64 p-1"
                trigger={({ open, toggle }) => (
                  <Button
                    size="sm"
                    onClick={toggle}
                    aria-expanded={open}
                    aria-label="More sync options"
                    className="-ml-px rounded-l-none px-1.5"
                  >
                    <span aria-hidden>▾</span>
                  </Button>
                )}
              >
                {({ close }) => (
                  <div>
                    <button
                      type="button"
                      disabled={reindexDisabled}
                      onClick={() => {
                        close();
                        onReindex();
                      }}
                      className="w-full rounded-[var(--radius)] px-2 py-1.5 text-left text-sm text-ink hover:bg-surface disabled:cursor-not-allowed disabled:text-ink4 disabled:hover:bg-transparent"
                    >
                      Re-index everything
                    </button>
                    {/* The reason sits in the menu, beside the control it explains. A greyed item
                        with no explanation is the thing users report as broken. */}
                    {reindexDisabled && reindexBlockedReason && (
                      <p className="px-2 pb-1 pt-0.5 text-xs text-ink4">{reindexBlockedReason}</p>
                    )}
                  </div>
                )}
              </Popover>
            )}
          </div>
          <Button
            variant="tertiary"
            size="sm"
            onClick={onAction}
            disabled={actionDisabled}
            className="hover:text-st-due"
          >
            {actionLabel}
          </Button>
        </div>
      </div>
      {children}
    </div>
  );
}
