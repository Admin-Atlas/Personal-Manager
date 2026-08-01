// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The Proton Drive destination's own state and actions.
//
// Instantiated by `BackupSettings`, not by `ProtonDriveSection`, and that is load-bearing rather
// than stylistic: `protonBusy` is one half of the panel-wide `busy` gate (the app-side mirror of
// the backend's BusyGuard, which is what makes "Disconnect is disabled during an upload it would
// kill" true), `protonConnected` is read by two OTHER sections — the "Save to Proton Drive…" button
// and the schedule's destination tick — and `doProtonDisconnect` is fired from the confirmation
// dialog, which lives at the panel root. Owning this state inside the section would drop the
// cross-destination half of that mutual exclusion silently.
//
// It is deliberately NOT one generic `useBackupDestination(kind)` shared with Google: the status
// models genuinely differ (a CLI install/locate flow and a window-focus re-probe here, an OAuth
// account picker there). What is actually identical between the two is the RENDER of the connected
// branch, and that is shared — see `CloudDestinationPanel`.

import { useCallback, useEffect, useState } from "react";

import {
  listProtonBackups,
  protonCliStatus,
  protonConnect,
  protonDisconnect,
  protonStatus,
  restoreFromProton,
  setProtonCliPath,
  backupToProton,
} from "../../lib/ipc";
import type {
  BackupEntry,
  ProtonCliStatus,
  ProtonConnStatus,
  RestoreSummary,
} from "../../lib/types";

/** The panel-wide sinks these handlers write into, plus the schedule reload a manual backup owes
 *  (it stamps "last backup" too). All owned by `BackupSettings`. */
export interface ProtonBackupDeps {
  setError: (m: string | null) => void;
  setMessage: (m: string | null) => void;
  setRunning: (v: boolean) => void;
  setRestored: (s: RestoreSummary | null) => void;
  refreshSchedule: () => Promise<void>;
}

export type UseProtonBackup = ReturnType<typeof useProtonBackup>;

export function useProtonBackup({
  setError,
  setMessage,
  setRunning,
  setRestored,
  refreshSchedule,
}: ProtonBackupDeps) {
  // Proton Drive destination. `proton` = CLI install status (null = still checking); `conn` =
  // session status once installed; `protonBackups` = archives already on Drive.
  const [proton, setProton] = useState<ProtonCliStatus | null>(null);
  const [conn, setConn] = useState<ProtonConnStatus | null>(null);
  const [protonBackups, setProtonBackups] = useState<BackupEntry[] | null>(null);
  const [protonBusy, setProtonBusy] = useState(false);
  const [protonRestoreName, setProtonRestoreName] = useState<string | null>(null);
  const [protonRestorePass, setProtonRestorePass] = useState("");
  const [protonListError, setProtonListError] = useState<string | null>(null);
  // "Locate manually…" state: an error if the user picked a non-CLI file; `locating` guards the pick.
  const [locateError, setLocateError] = useState<string | null>(null);
  const [locating, setLocating] = useState(false);

  const refreshProton = useCallback(async () => {
    const s = await protonCliStatus().catch(() => null);
    setProton(s);
    if (!s?.installed) {
      setConn(null);
      setProtonBackups(null);
      return;
    }
    const c = await protonStatus().catch(() => null);
    setConn(c);
    if (c?.connected) {
      // Distinguish "still loading" (null) from "failed to load" (protonListError), so the list
      // can't get stuck on "Loading…" forever when the CLI errors.
      try {
        setProtonBackups(await listProtonBackups());
        setProtonListError(null);
      } catch (e) {
        setProtonBackups(null);
        setProtonListError(String(e));
      }
    } else {
      setProtonBackups(null);
      setProtonListError(null);
    }
  }, []);

  // "Locate manually…": let the user point PM at the proton-drive binary wherever it lives (the CLI
  // is a portable single file), remember it, and re-probe. The backend rejects a non-file path.
  const locateCli = useCallback(async () => {
    if (locating) return;
    setLocating(true);
    setLocateError(null);
    try {
      // The backend opens the file picker itself (L-5: the stored path is spawned as a subprocess,
      // so it must never be a webview-supplied string). Cancelling is a no-op there.
      await setProtonCliPath();
      await refreshProton();
    } catch (e) {
      setLocateError(String(e));
    } finally {
      setLocating(false);
    }
  }, [locating, refreshProton]);

  useEffect(() => {
    void refreshProton();
  }, [refreshProton]);
  // Re-probe when the window regains focus while the CLI still isn't detected, so installing it
  // while PM is open is picked up without a restart. Gated to the not-found case so it never
  // re-spawns the CLI (a `proton_status` call) on every focus once it's already located.
  useEffect(() => {
    if (proton?.installed) return;
    const onFocus = () => void refreshProton();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [proton?.installed, refreshProton]);

  const protonConnected = !!(proton?.installed && conn?.connected);

  async function doProtonConnect() {
    setError(null);
    setMessage(null);
    setProtonBusy(true);
    try {
      await protonConnect(); // opens the browser; resolves once sign-in completes
      await refreshProton();
    } catch (e) {
      setError(String(e));
    } finally {
      setProtonBusy(false);
    }
  }

  async function doProtonDisconnect() {
    setError(null);
    setProtonBusy(true);
    try {
      await protonDisconnect();
      await refreshProton();
    } catch (e) {
      setError(String(e));
    } finally {
      setProtonBusy(false);
    }
  }

  // The typed passphrase is supplied by the caller rather than held here: it is plaintext, it lives
  // in exactly one closure (the panel root's), and widening its lifetime to a destination hook
  // would be the only thing this split made worse.
  async function doProtonBackup(passphrase: string) {
    setError(null);
    setMessage(null);
    try {
      setRunning(true);
      await backupToProton(passphrase);
      setMessage(
        "Backup uploaded to Proton Drive. Keep your passphrase safe — you need it to restore.",
      );
      await refreshProton();
      await refreshSchedule(); // a manual Proton backup stamps "last backup" too
    } catch (e) {
      setError(String(e));
    } finally {
      setRunning(false);
    }
  }

  async function doProtonRestore() {
    if (!protonRestoreName || protonRestorePass.length === 0) return;
    setError(null);
    setMessage(null);
    setRestored(null);
    try {
      setRunning(true);
      const summary = await restoreFromProton(protonRestoreName, protonRestorePass);
      setRestored(summary);
      setProtonRestorePass("");
      setProtonRestoreName(null);
    } catch (e) {
      setError(String(e));
    } finally {
      setRunning(false);
    }
  }

  return {
    proton,
    conn,
    protonBackups,
    protonBusy,
    // Taken by the panel root's "Delete oldest" action, which is shared with Google and so lives
    // above both destinations.
    setProtonBusy,
    protonRestoreName,
    setProtonRestoreName,
    protonRestorePass,
    setProtonRestorePass,
    protonListError,
    locateError,
    locating,
    refreshProton,
    locateCli,
    protonConnected,
    doProtonConnect,
    doProtonDisconnect,
    doProtonBackup,
    doProtonRestore,
  };
}
