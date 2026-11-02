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

import { useCallback, useEffect, useRef, useState } from "react";

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
  // Settings saves as you go, and so does this: the frequency the moment it's chosen, "Keep last"
  // once you're done typing it (you leave the field or press Enter). Not per keystroke — typing 15
  // passes through 1, and a scheduled run in between would prune to one backup. While it's being
  // typed it's the one unsaved thing in Settings, so it registers: leaving the tab, or closing
  // Settings, commits it first instead of the unmount discarding it.
  const scheduleDirty =
    schedule != null &&
    (freqDraft !== schedule.frequency || retentionDraft !== String(schedule.retention_n));
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
    async () => {
      // Throws, unlike the field's own commit, so a failed save keeps Settings where its error shows.
      if (!(await saveSchedule(freqDraft, retentionDraft))) {
        throw new Error("The backup schedule wasn't saved.");
      }
    },
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

  // The save in flight and what it writes, so the field's blur and the leave-the-tab commit that
  // follows it in the same click share one write instead of racing two. A save of other values
  // waits for it rather than being dropped.
  const inFlight = useRef<{ key: string; run: Promise<boolean> } | null>(null);

  /** Write the schedule, resolving whether it was saved. Takes the values rather than reading the
   *  drafts, so a frequency just chosen is saved before its state update has rendered. */
  function saveSchedule(
    frequency: BackupSchedule["frequency"],
    retention: string,
  ): Promise<boolean> {
    const key = `${frequency}|${retention}`;
    if (inFlight.current?.key === key) return inFlight.current.run;
    const before = inFlight.current?.run;
    const entry: { key: string; run: Promise<boolean> } = { key, run: Promise.resolve(false) };
    entry.run = (async () => {
      if (before) await before;
      setMessage(null);
      setScheduleSaveError(null);
      setSavingSchedule(true);
      try {
        // Round + clamp the free-typed retention so the backend's u32 never sees a non-integer.
        const retentionN = Math.max(1, Math.min(100, Math.round(Number(retention) || 5)));
        await setBackupSchedule(frequency, retentionN);
        setMessage(
          frequency === "off"
            ? "Automatic backups turned off."
            : "Automatic backup schedule saved.",
        );
        await refreshSchedule();
        return true;
      } catch (e) {
        setScheduleSaveError(String(e));
        return false;
      } finally {
        setSavingSchedule(false);
        if (inFlight.current === entry) inFlight.current = null;
      }
    })();
    inFlight.current = entry;
    return entry.run;
  }

  /** A frequency chosen: saved at once. */
  function changeFrequency(next: BackupSchedule["frequency"]) {
    setFreqDraft(next);
    void saveSchedule(next, retentionDraft);
  }

  /** "Keep last" left or confirmed with Enter: saved if it changed. */
  function commitRetention() {
    if (!schedule || retentionDraft === String(schedule.retention_n)) return;
    void saveSchedule(freqDraft, retentionDraft);
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
    changeFrequency,
    retentionDraft,
    setRetentionDraft,
    commitRetention,
    savingSchedule,
    refreshSchedule,
    passphraseStored,
    showStatus,
    enabledDestinations,
    keepN,
    doForgetPass,
    doToggleDestination,
  };
}
