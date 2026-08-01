// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The automatic-backup schedule: the loaded record, its two editable drafts, and every write that
// touches it.
//
// Instantiated by `BackupSettings`, NOT by `AutomaticBackupsSection`, because the schedule is the
// most-shared object on this tab — `passphrase_stored` gates the passphrase section,
// `retention_n` feeds both destination panels, `savingSchedule` is read as a `disabled` on the
// passphrase section's Forget button, and `scheduleSaveError` is written by the Forget dialog and
// by both destinations' reconciliation banners while rendering only inside the schedule section.
// Keeping the load effect and `useRegisterPending` here (above the section's own
// `scheduleError ? … : schedule === null ? …` branch) is also what stops the unsaved-draft guard
// clearing itself whenever the schedule is null or failed to load.

import { useCallback, useEffect, useState } from "react";

import {
  forgetBackupPassphrase,
  getBackupSchedule,
  setBackupDestinations,
  setBackupSchedule,
} from "../../lib/ipc";
import type { BackupSchedule } from "../../lib/types";
import { useRegisterPending } from "../../lib/settingsPending";

/** The panel-wide outcome sink this hook writes into. It lives in `BackupSettings` because the tab
 *  renders one message line for every section. */
export interface BackupScheduleDeps {
  setMessage: (m: string | null) => void;
}

export type UseBackupSchedule = ReturnType<typeof useBackupSchedule>;

export function useBackupSchedule({ setMessage }: BackupScheduleDeps) {
  // Automatic-backup schedule + status. Loaded on mount (it only needs an unlocked vault, not
  // Proton), so the status summary and passphrase state are known before you connect. Drafts are
  // the editable form values; `retentionDraft` is a string so the number field can be cleared.
  const [schedule, setSchedule] = useState<BackupSchedule | null>(null);
  const [scheduleError, setScheduleError] = useState<string | null>(null);
  const [scheduleSaveError, setScheduleSaveError] = useState<string | null>(null);
  const [freqDraft, setFreqDraft] = useState<BackupSchedule["frequency"]>("off");
  const [retentionDraft, setRetentionDraft] = useState("5");
  const [savingSchedule, setSavingSchedule] = useState(false);

  // Schedule + passphrase-stored state is independent of Proton, so load it on its own.
  const refreshSchedule = useCallback(async () => {
    try {
      const sch = await getBackupSchedule();
      setSchedule(sch);
      setScheduleError(null);
      setFreqDraft(sch.frequency);
      setRetentionDraft(String(sch.retention_n));
    } catch (e) {
      // A keychain/DB hiccup must not leave the panel stuck on "Loading…" forever.
      setSchedule(null);
      setScheduleError(String(e));
    }
  }, []);

  useEffect(() => {
    void refreshSchedule();
  }, [refreshSchedule]);

  const passphraseStored = schedule?.passphrase_stored ?? false;
  const showStatus = !!schedule && (schedule.frequency !== "off" || !!schedule.last_backup_at);
  // The destinations a scheduled run would push to, for the status summary line.
  const enabledDestinations = [
    schedule?.proton_enabled ? "Proton Drive" : null,
    schedule?.gdrive_enabled ? "Google Drive" : null,
  ].filter(Boolean) as string[];

  const keepN = schedule?.retention_n ?? null;
  // The one control in Settings that holds an unsaved buffer. Done — or merely switching Settings
  // tabs, which unmounts this panel — throws the drafts away silently, which made the footer's
  // "changes are saved as you make them" untrue here. Say so instead of pretending.
  const scheduleDirty =
    schedule != null &&
    (freqDraft !== schedule.frequency || retentionDraft !== String(schedule.retention_n));
  // Register those drafts so leaving the tab, or closing Settings, asks first and names them —
  // rather than the unmount discarding them in silence. Only the fields that actually differ are
  // listed, so the dialog reads as a specific warning rather than a generic one.
  useRegisterPending(
    "backup-schedule",
    "backup",
    scheduleDirty,
    [
      schedule && freqDraft !== schedule.frequency ? "Backup frequency" : null,
      schedule && retentionDraft !== String(schedule.retention_n)
        ? "How many backups to keep"
        : null,
    ].filter((x): x is string => x !== null),
    doSaveSchedule,
  );

  async function doForgetPass() {
    setMessage(null);
    setScheduleSaveError(null);
    setSavingSchedule(true);
    try {
      await forgetBackupPassphrase();
      await refreshSchedule();
      // Narrate the side effect, don't just warn about it beforehand. The command turns the cadence
      // off as well as dropping the secret (deliberately — see `describeForgetConsequences`), and
      // this used to say nothing at all on success: the only feedback was the panel mutating.
      setMessage(
        "Passphrase forgotten. Automatic backups are off; the backups you already have are unchanged.",
      );
    } catch (e) {
      setScheduleSaveError(String(e));
    } finally {
      setSavingSchedule(false);
    }
  }

  // Toggle a destination on/off for scheduled runs (saved immediately). Enabling Google requires a
  // granted account; the backend rejects otherwise, so the toggle is disabled until it's granted.
  async function doToggleDestination(which: "proton" | "gdrive", next: boolean) {
    if (!schedule) return;
    setMessage(null);
    setScheduleSaveError(null);
    setSavingSchedule(true);
    try {
      const protonEnabled = which === "proton" ? next : schedule.proton_enabled;
      const gdriveEnabled = which === "gdrive" ? next : schedule.gdrive_enabled;
      await setBackupDestinations(protonEnabled, gdriveEnabled);
      await refreshSchedule();
    } catch (e) {
      setScheduleSaveError(String(e));
    } finally {
      setSavingSchedule(false);
    }
  }

  async function doSaveSchedule() {
    setMessage(null);
    setScheduleSaveError(null);
    setSavingSchedule(true);
    try {
      // Round + clamp the free-typed retention so the backend's u32 never sees a non-integer.
      const retentionN = Math.max(1, Math.min(100, Math.round(Number(retentionDraft) || 5)));
      await setBackupSchedule(freqDraft, retentionN);
      setMessage(
        freqDraft === "off" ? "Automatic backups turned off." : "Automatic backup schedule saved.",
      );
      await refreshSchedule();
    } catch (e) {
      setScheduleSaveError(String(e));
    } finally {
      setSavingSchedule(false);
    }
  }

  return {
    schedule,
    scheduleError,
    scheduleSaveError,
    // Written from outside this hook by the reconciliation banners' "Keep all N" action, which is
    // a SCHEDULE write fired from a destination panel. Exposed rather than hidden so that coupling
    // stays visible instead of being re-invented behind a second sink.
    setScheduleSaveError,
    freqDraft,
    setFreqDraft,
    retentionDraft,
    setRetentionDraft,
    savingSchedule,
    refreshSchedule,
    passphraseStored,
    showStatus,
    enabledDestinations,
    keepN,
    scheduleDirty,
    doForgetPass,
    doToggleDestination,
    doSaveSchedule,
  };
}
