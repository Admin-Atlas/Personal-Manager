// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Automatic backups — one schedule fans out to every destination you turn on.
//
// The schedule state, its load effect and the unsaved-draft registration all live in
// `useBackupSchedule` at the panel root rather than here, because this section renders NOTHING at
// all while the schedule is loading or failed, and a registration that unmounts with the form is a
// draft guard that dies exactly when it is needed.

import type { BackupSchedule } from "../../lib/types";
import { formatDateTime } from "../../lib/format";
import { Button, SectionInfo, SectionLabel, Select } from "../ui";
import type { UseBackupSchedule } from "./useBackupSchedule";

export interface AutomaticBackupsSectionProps {
  state: UseBackupSchedule;
  /** The panel-wide "any op in flight" gate — `running || protonBusy || gdriveBusy`. */
  busy: boolean;
  protonConnected: boolean;
  gdriveGranted: boolean;
}

export function AutomaticBackupsSection({
  state,
  busy,
  protonConnected,
  gdriveGranted,
}: AutomaticBackupsSectionProps) {
  const {
    schedule,
    scheduleError,
    scheduleSaveError,
    freqDraft,
    setFreqDraft,
    retentionDraft,
    setRetentionDraft,
    savingSchedule,
    refreshSchedule,
    passphraseStored,
    enabledDestinations,
    scheduleDirty,
    doToggleDestination,
    doSaveSchedule,
  } = state;

  return (
    <div className="mt-6 border-t border-border pt-4">
      <SectionLabel>Automatic backups</SectionLabel>
      {scheduleError ? (
        <div className="mt-2 flex items-center gap-2">
          <span className="text-xs text-st-due">Couldn&rsquo;t load the schedule.</span>
          <Button variant="tertiary" onClick={() => void refreshSchedule()} disabled={busy}>
            Retry
          </Button>
        </div>
      ) : schedule === null ? (
        <p className="mt-2 text-xs text-ink4">Loading&hellip;</p>
      ) : (
        <div className="mt-2 flex max-w-sm flex-col gap-3">
          <div className="flex flex-col gap-2">
            <label className="flex items-center gap-2 text-xs text-ink3">
              <input
                type="checkbox"
                checked={schedule.proton_enabled}
                onChange={(e) => void doToggleDestination("proton", e.currentTarget.checked)}
                disabled={savingSchedule || busy}
              />
              <span>Back up to Proton Drive</span>
              {!protonConnected && <span className="text-ink4">(connect above to use)</span>}
            </label>
            <label className="flex items-center gap-2 text-xs text-ink3">
              <input
                type="checkbox"
                checked={schedule.gdrive_enabled}
                onChange={(e) => void doToggleDestination("gdrive", e.currentTarget.checked)}
                disabled={savingSchedule || busy || !gdriveGranted}
              />
              <span>Back up to Google Drive</span>
              {!gdriveGranted && <span className="text-ink4">(grant access above to use)</span>}
            </label>
          </div>

          <label className="flex items-center justify-between gap-2 text-xs text-ink3">
            <span>Frequency</span>
            <Select
              compact
              value={freqDraft}
              onChange={(e) => setFreqDraft(e.currentTarget.value as BackupSchedule["frequency"])}
              disabled={savingSchedule || busy}
            >
              <option value="off">Off</option>
              <option value="daily">Daily</option>
              <option value="weekly">Weekly</option>
              <option value="monthly">Monthly</option>
            </Select>
          </label>

          {freqDraft !== "off" && (
            <label className="flex items-center justify-between gap-2 text-xs text-ink3">
              <span>Keep last</span>
              <input
                type="number"
                min={1}
                max={100}
                className="w-20 rounded-[var(--radius-sm)] border border-border bg-surface px-2 py-1 text-ink2"
                value={retentionDraft}
                onChange={(e) => setRetentionDraft(e.currentTarget.value)}
                disabled={savingSchedule || busy}
              />
            </label>
          )}

          {freqDraft !== "off" && !passphraseStored && (
            <p className="text-xs text-st-due">
              Remember your backup passphrase above (under &ldquo;Backup passphrase&rdquo;) to turn
              on automatic backups.
            </p>
          )}

          {freqDraft !== "off" && passphraseStored && enabledDestinations.length === 0 && (
            <p className="text-xs text-st-due">
              Turn on at least one destination above for scheduled backups to run.
            </p>
          )}

          <div className="flex items-center gap-2">
            <Button
              variant="secondary"
              onClick={doSaveSchedule}
              disabled={savingSchedule || busy || (freqDraft !== "off" && !passphraseStored)}
            >
              {savingSchedule ? "Saving…" : "Save schedule"}
            </Button>
            {scheduleDirty && !savingSchedule && (
              <span className="text-xs text-st-look">Not saved yet</span>
            )}
          </div>

          {scheduleSaveError && (
            <p className="break-words text-xs text-st-due">{scheduleSaveError}</p>
          )}

          {schedule.last_backup_at && (
            <p className="text-xs text-ink4">
              Last automatic backup: {formatDateTime(schedule.last_backup_at)}
            </p>
          )}
        </div>
      )}
      <SectionInfo title="How automatic backups work">
        <p>
          PM backs up your current vault on a schedule, using the passphrase you remembered above,
          to whichever destinations you turn on here.
        </p>
      </SectionInfo>
    </div>
  );
}
