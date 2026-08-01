// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Backup tab's status summary — so reopening the app shows where backups stand at a glance.

import type { BackupSchedule } from "../../lib/types";
import { formatDateTime } from "../../lib/format";
import { BACKUP_FREQUENCY_LABEL } from "../../lib/backup";

export interface BackupStatusSummaryProps {
  /** `showStatus`: there is a schedule, and it either runs or has already run once. */
  show: boolean;
  schedule: BackupSchedule | null;
  /** The destinations a scheduled run would push to. */
  enabledDestinations: string[];
}

export function BackupStatusSummary({
  show,
  schedule,
  enabledDestinations,
}: BackupStatusSummaryProps) {
  if (!show || !schedule) return null;
  return (
    <div className="mt-3 max-w-sm rounded-[var(--radius-sm)] border border-border2 bg-surface p-3">
      <p className="font-mono text-xs uppercase tracking-wide text-ink3">Backup status</p>
      <dl className="mt-1.5 flex flex-col gap-1 text-xs text-ink3">
        <div className="flex justify-between gap-2">
          <dt className="text-ink4">Automatic</dt>
          <dd className="text-right">
            {schedule.frequency === "off"
              ? "Off"
              : `${BACKUP_FREQUENCY_LABEL[schedule.frequency]} → ${
                  enabledDestinations.length ? enabledDestinations.join(", ") : "no destination"
                }`}
          </dd>
        </div>
        {schedule.frequency !== "off" && (
          <div className="flex justify-between gap-2">
            <dt className="text-ink4">Keeping</dt>
            <dd className="text-right">last {schedule.retention_n}</dd>
          </div>
        )}
        <div className="flex justify-between gap-2">
          <dt className="text-ink4">Last backup</dt>
          <dd className="text-right">
            {schedule.last_backup_at ? formatDateTime(schedule.last_backup_at) : "None yet"}
          </dd>
        </div>
      </dl>
    </div>
  );
}
