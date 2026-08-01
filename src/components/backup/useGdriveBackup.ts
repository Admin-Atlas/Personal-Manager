// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Google Drive destination's own state and actions (Drive v3 REST).
//
// Instantiated by `BackupSettings` for the same three reasons as its Proton twin: `gdriveBusy` is
// one half of the panel-wide `busy` gate, `gdriveGranted` is read by two other sections, and both
// `gdriveAlsoConnector` (which decides what the disconnect confirmation says) and
// `doGdriveDisconnect` are needed by the dialog at the panel root.

import { useCallback, useEffect, useState } from "react";

import {
  backupGdriveConnect,
  backupGdriveDisconnect,
  backupGdriveStatus,
  backupToGdrive,
  listGdriveBackups,
  restoreFromGdrive,
} from "../../lib/ipc";
import type { BackupEntry, GdriveBackupStatus, RestoreSummary } from "../../lib/types";

/** The panel-wide sinks these handlers write into, plus the schedule reload a manual backup owes
 *  (it stamps "last backup" too). All owned by `BackupSettings`. */
export interface GdriveBackupDeps {
  setError: (m: string | null) => void;
  setMessage: (m: string | null) => void;
  setRunning: (v: boolean) => void;
  setRestored: (s: RestoreSummary | null) => void;
  refreshSchedule: () => Promise<void>;
}

export type UseGdriveBackup = ReturnType<typeof useGdriveBackup>;

export function useGdriveBackup({
  setError,
  setMessage,
  setRunning,
  setRestored,
  refreshSchedule,
}: GdriveBackupDeps) {
  // Google Drive destination (Drive v3 REST). `gdrive` = null while checking; once loaded,
  // `has_write_scope` gates the whole panel (a drive.file re-consent is required — connector
  // scopes are read-only). `gdriveBackups` = archives already on Drive.
  const [gdrive, setGdrive] = useState<GdriveBackupStatus | null>(null);
  const [gdriveBackups, setGdriveBackups] = useState<BackupEntry[] | null>(null);
  const [gdriveBusy, setGdriveBusy] = useState(false);
  const [gdriveRestoreName, setGdriveRestoreName] = useState<string | null>(null);
  const [gdriveRestorePass, setGdriveRestorePass] = useState("");
  const [gdriveListError, setGdriveListError] = useState<string | null>(null);
  const [gdriveAccountChoice, setGdriveAccountChoice] = useState("");

  // Google Drive status is independent of Proton/schedule, so load it on its own.
  const refreshGdrive = useCallback(async () => {
    const s = await backupGdriveStatus().catch(() => null);
    setGdrive(s);
    if (s?.has_write_scope) {
      try {
        setGdriveBackups(await listGdriveBackups());
        setGdriveListError(null);
      } catch (e) {
        setGdriveBackups(null);
        setGdriveListError(String(e));
      }
    } else {
      setGdriveBackups(null);
      setGdriveListError(null);
    }
  }, []);

  useEffect(() => {
    void refreshGdrive();
  }, [refreshGdrive]);

  const gdriveGranted = !!gdrive?.has_write_scope;
  // Whether the account set up for BACKUP is also connected as a read-only Drive source. It decides
  // what disconnecting costs: the backend only deletes the keychain token when the account is not
  // also a connector (`backup_gdrive_disconnect`), so one case keeps working and the other needs a
  // fresh `drive.file` grant. Case-insensitive, matching the backend's `eq_ignore_ascii_case`.
  const gdriveAccount = gdrive?.account ?? null;
  const gdriveAlsoConnector =
    gdriveAccount !== null &&
    (gdrive?.accounts ?? []).some((a) => a.email.toLowerCase() === gdriveAccount.toLowerCase());

  async function doGdriveConnect(email?: string) {
    setError(null);
    setMessage(null);
    setGdriveBusy(true);
    try {
      await backupGdriveConnect(email); // opens the browser; resolves once consent completes
      await refreshGdrive();
      await refreshSchedule();
      setMessage("Google Drive is set up for backup.");
    } catch (e) {
      setError(String(e));
    } finally {
      setGdriveBusy(false);
    }
  }

  async function doGdriveDisconnect() {
    setError(null);
    setGdriveBusy(true);
    try {
      await backupGdriveDisconnect();
      await refreshGdrive();
      await refreshSchedule();
    } catch (e) {
      setError(String(e));
    } finally {
      setGdriveBusy(false);
    }
  }

  // The typed passphrase is supplied by the caller rather than held here — see the note on
  // `doProtonBackup`.
  async function doGdriveBackup(passphrase: string) {
    setError(null);
    setMessage(null);
    try {
      setRunning(true);
      await backupToGdrive(passphrase);
      setMessage(
        "Backup uploaded to Google Drive. Keep your passphrase safe — you need it to restore.",
      );
      await refreshGdrive();
      await refreshSchedule(); // a manual backup stamps "last backup" too
    } catch (e) {
      setError(String(e));
    } finally {
      setRunning(false);
    }
  }

  async function doGdriveRestore() {
    if (!gdriveRestoreName || gdriveRestorePass.length === 0) return;
    setError(null);
    setMessage(null);
    setRestored(null);
    try {
      setRunning(true);
      const summary = await restoreFromGdrive(gdriveRestoreName, gdriveRestorePass);
      setRestored(summary);
      setGdriveRestorePass("");
      setGdriveRestoreName(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setRunning(false);
    }
  }

  return {
    gdrive,
    gdriveBackups,
    gdriveBusy,
    // Taken by the panel root's "Delete oldest" action, which is shared with Proton and so lives
    // above both destinations.
    setGdriveBusy,
    gdriveRestoreName,
    setGdriveRestoreName,
    gdriveRestorePass,
    setGdriveRestorePass,
    gdriveListError,
    gdriveAccountChoice,
    setGdriveAccountChoice,
    refreshGdrive,
    gdriveGranted,
    gdriveAlsoConnector,
    doGdriveConnect,
    doGdriveDisconnect,
    doGdriveBackup,
    doGdriveRestore,
  };
}
