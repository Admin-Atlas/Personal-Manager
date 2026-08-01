// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { BackupPhase, BackupSchedule } from "./types";

// Small, UI-free helpers for the Backup panel — pulled out of the component so the pure logic
// (phase classification, the partial-failure banner copy) is unit-tested on its own.

/**
 * Phases whose progress fraction isn't a true measure of the work done, so the bar should show an
 * indeterminate shimmer rather than a percentage (F-45). The upload/download fraction is a coarse
 * per-destination fan-out step (0 then 1 for a single destination) — the Proton CLI has no byte
 * callback and the Google resumable upload isn't metered — so a percent bar sits frozen at 0%
 * through a minutes-long transfer. Matches the design system's "opaque phases always shimmer" rule.
 */
export function isOpaquePhase(phase: BackupPhase | null): boolean {
  return phase === "upload" || phase === "download";
}

/**
 * Copy for the non-blocking "backed up, but some destinations failed" banner (F-22). A backup run
 * fans out to every enabled destination and stamps success per-destination; when at least one
 * succeeds but others fail, the backend returns the failures as `"<label>: <error>"` strings. Null
 * when nothing failed (the clean-run case), so the caller can `setWarning(describeFailures(...))`
 * directly.
 */
export function describeFailures(failed: readonly string[]): string | null {
  if (failed.length === 0) return null;
  const noun = failed.length === 1 ? "destination" : "destinations";
  return `Backed up, but ${failed.length} ${noun} failed — ${failed.join("; ")}. The archive did reach the destinations that succeeded.`;
}

/** How a cadence is written wherever the panel names one. Lives here rather than in the component so
 *  the schedule readout and the Forget confirmation can't drift into two spellings of "Weekly". */
export const BACKUP_FREQUENCY_LABEL: Record<BackupSchedule["frequency"], string> = {
  off: "Off",
  daily: "Daily",
  weekly: "Weekly",
  monthly: "Monthly",
};

/**
 * The SECOND consequence of forgetting the backup passphrase, in the user's own cadence — or null
 * when there is no schedule to lose.
 *
 * Forgetting doesn't only drop the secret: `forget_backup_passphrase` writes the cadence to `off`
 * first, deliberately, so a failure between its two writes can never leave the scheduler with a
 * cadence and no passphrase. That ordering is right and must not be "fixed"; what was wrong is that
 * nothing told the user it happened. Conditional because a false alarm is its own defect — someone
 * with no schedule shouldn't be warned about losing one.
 */
export function describeForgetConsequences(frequency: BackupSchedule["frequency"]): string | null {
  if (frequency === "off") return null;
  return (
    `Automatic backups are ${BACKUP_FREQUENCY_LABEL[frequency]} — this switches them to Off. ` +
    `Your destinations, how many backups to keep, and every backup file you already have are ` +
    `untouched, but nothing new is backed up until you remember a passphrase again and turn the ` +
    `schedule back on.`
  );
}
