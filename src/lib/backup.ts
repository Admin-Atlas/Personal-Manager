// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { BackupPhase, BackupSchedule, RetentionNote } from "./types";

// Small, UI-free helpers for the Backup panel — pulled out of the component so the pure logic
// (phase classification, the partial-failure banner copy) is unit-tested on its own.

/**
 * Phases whose progress fraction isn't a true measure of the work done, so the bar should show an
 * indeterminate shimmer rather than a percentage (F-45). Matches the design system's "opaque
 * phases always shimmer" rule.
 *
 * - `upload`/`download`: the fraction is a coarse per-destination fan-out step (0 then 1 for a
 *   single destination) — the Proton CLI has no byte callback and the Google resumable upload
 *   isn't metered — so a percent bar sits frozen at 0% through a minutes-long transfer.
 * - `snapshot`: `VACUUM INTO` is a single opaque SQLite call. `begin_backup_run` opens the run on
 *   `Snapshot`, and the only other emission is `fraction: 1.0` once the vacuum returns
 *   (commands/backups.rs:122-129 and :765-772) — nothing in between. On a large store this is the
 *   longest visible stretch of a backup and it was rendering as a bar pinned at 0%.
 * - `validate`: identical shape — `restore.rs:201` reports 0.0 and `:238` reports 1.0.
 *
 * `pack` and `restore` are the genuinely metered ones: they are the only two that read through
 * `ProgressReader` (pack.rs:127/:140, restore.rs:134), so their fractions are real byte counts.
 */
export function isOpaquePhase(phase: BackupPhase | null): boolean {
  return phase === "upload" || phase === "download" || phase === "snapshot" || phase === "validate";
}

/**
 * The creation instant encoded in a PM archive name, as an extended-ISO string — or null for a name
 * that isn't ours.
 *
 * The backend names archives `pm-backup-<vault-id>-<YYYYMMDDTHHMMSSZ>.pmbackup` (backup/naming.rs),
 * and that shape is load-bearing: it is the retention sort key AND the argument the restore command
 * takes, so it must not be renamed to make the list readable. The list is fixed by READING the
 * stamp instead.
 *
 * The stamp must be expanded before it reaches `format.ts`: `new Date("20260801T161659Z")` is
 * `Invalid Date`, because JS parses only extended ISO. Returns null rather than throwing for a
 * foreign `.pmbackup` sitting in the same folder — the listing filters on extension alone, so the
 * caller must still render those rows, just without a parsed date.
 */
export function archiveStampIso(name: string): string | null {
  const m = /-(\d{4})(\d{2})(\d{2})T(\d{2})(\d{2})(\d{2})Z\.pmbackup$/.exec(name);
  return m ? `${m[1]}-${m[2]}-${m[3]}T${m[4]}:${m[5]}:${m[6]}Z` : null;
}

/**
 * A compact UTC stamp `YYYYMMDDTHHMMSSZ`, the same shape the backend gives cloud archives — used
 * for the local "Save a backup" default filename so a folder of local saves is orderable and a
 * second save doesn't offer to overwrite the first. Deliberately NOT the DD-MM-YYYY house format:
 * this is a filename, where colon-free and lexically sortable beats readable.
 */
export function localSaveStamp(now: Date = new Date()): string {
  return `${now
    .toISOString()
    .replace(/[-:]/g, "")
    .replace(/\.\d{3}/, "")}`;
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

/**
 * Which stored retention notes are still true, given what a fresh listing says.
 *
 * A keep-last-N note is the one half of a run report PM can honestly re-derive: if the destination
 * is now back under its limit, the archives were trimmed or removed and the note is spent. That is
 * what makes deleting the extras in Drive clear the message on the next visit, with no new IPC and
 * no polling.
 *
 * `stillOverLimit` returns `true` (still over), `false` (confirmed under) or **`null` for unknown**
 * — and unknown must KEEP the note. This is the whole safety property: the caller's count is null
 * whenever the listing has not loaded yet, the request threw, or the write scope is missing, and in
 * every one of those cases "not over the limit" is an absence of evidence, not evidence of absence.
 * Suppressing on null would hide a true warning exactly when PM can least see the destination.
 *
 * Notes with `over_limit: false` are transport failures ("trimming old backups failed") and are
 * never suppressed by a count — a listing that succeeds says nothing about whether the trim would.
 */
export function visibleRetentionNotes(
  notes: readonly RetentionNote[],
  stillOverLimit: (kind: string) => boolean | null,
): RetentionNote[] {
  return notes.filter((n) => !(n.over_limit && stillOverLimit(n.kind) === false));
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
