// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Settings → Backup. This surface owns the whole encrypted-backup feature (deliberately NOT in
// Connectors — a backup is a push-out snapshot, not an index-only source). The tab reads as a
// guided flow: (1) choose the passphrase that locks every backup, optionally remembering it on the
// device for unattended runs; (2) save a backup now — to this computer, Proton Drive, or Google
// Drive; restore from a file or either cloud; and (3) schedule automatic backups on one cadence
// that fans out to every destination you turn on. A status summary up top shows where things stand
// on every launch.

import { useCallback, useEffect, useRef, useState } from "react";
import { open as openFileDialog, save as saveFileDialog } from "@tauri-apps/plugin-dialog";

import {
  backupGdriveConnect,
  backupGdriveDisconnect,
  backupGdriveStatus,
  backupStatus,
  backupToGdrive,
  backupToProton,
  createLocalBackup,
  forgetBackupPassphrase,
  getBackupSchedule,
  listGdriveBackups,
  listProtonBackups,
  onBackupProgress,
  openUrl,
  protonCliStatus,
  protonConnect,
  protonDisconnect,
  protonStatus,
  restoreFromGdrive,
  restoreFromProton,
  restoreLocalBackup,
  setBackupDestinations,
  setBackupPassphrase,
  setBackupSchedule,
  stopBackup,
  switchToVault,
} from "../lib/ipc";
import type {
  BackupEntry,
  BackupPhase,
  BackupSchedule,
  GdriveBackupStatus,
  ProtonCliStatus,
  ProtonConnStatus,
  RestoreSummary,
} from "../lib/types";
import { formatDateTime } from "../lib/format";
import { Button, Collapsible, Input } from "./ui";
import { IngestProgress } from "./IngestProgress";
import { useDepth } from "../theme";

const PHASE_LABEL: Record<BackupPhase, string> = {
  snapshot: "Preparing a snapshot",
  pack: "Compressing & encrypting",
  upload: "Uploading",
  download: "Downloading",
  restore: "Decrypting & unpacking",
  validate: "Verifying",
};

const FREQ_LABEL: Record<BackupSchedule["frequency"], string> = {
  off: "Off",
  daily: "Daily",
  weekly: "Weekly",
  monthly: "Monthly",
};

/** A tiny, honest strength hint — length first (the biggest factor for a passphrase), with a
 *  nudge toward variety. Not a security oracle; just discourages a 4-char passphrase. */
function strength(pass: string): { label: string; tone: string } {
  if (pass.length === 0) return { label: "", tone: "" };
  const classes =
    (/[a-z]/.test(pass) ? 1 : 0) +
    (/[A-Z]/.test(pass) ? 1 : 0) +
    (/[0-9]/.test(pass) ? 1 : 0) +
    (/[^A-Za-z0-9]/.test(pass) ? 1 : 0);
  if (pass.length < 8) return { label: "Too short", tone: "text-st-due" };
  if (pass.length >= 16 || (pass.length >= 12 && classes >= 3))
    return { label: "Strong", tone: "text-st-quick" };
  return { label: "OK — longer is better", tone: "text-ink3" };
}

export function BackupSettings() {
  const { showPower } = useDepth();

  // Live progress (from the global backup://progress event + a status snapshot on mount).
  const [running, setRunning] = useState(false);
  const [phase, setPhase] = useState<BackupPhase | null>(null);
  const [fraction, setFraction] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);

  // The passphrase that locks every backup (manual saves read it directly; "Remember on this
  // device" additionally stows it in the keychain for unattended scheduled runs).
  const [pass, setPass] = useState("");
  const [confirm, setConfirm] = useState("");

  // Restore form.
  const [restoreSrc, setRestoreSrc] = useState<string | null>(null);
  const [restorePass, setRestorePass] = useState("");
  const [restored, setRestored] = useState<RestoreSummary | null>(null);
  const [switching, setSwitching] = useState(false);

  // Proton Drive destination. `proton` = CLI install status (null = still checking); `conn` =
  // session status once installed; `protonBackups` = archives already on Drive.
  const [proton, setProton] = useState<ProtonCliStatus | null>(null);
  const [conn, setConn] = useState<ProtonConnStatus | null>(null);
  const [protonBackups, setProtonBackups] = useState<BackupEntry[] | null>(null);
  const [protonBusy, setProtonBusy] = useState(false);
  const [protonRestoreName, setProtonRestoreName] = useState<string | null>(null);
  const [protonRestorePass, setProtonRestorePass] = useState("");
  const [protonListError, setProtonListError] = useState<string | null>(null);

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
    void refreshProton();
  }, [refreshProton]);
  useEffect(() => {
    void refreshGdrive();
  }, [refreshGdrive]);
  useEffect(() => {
    void refreshSchedule();
  }, [refreshSchedule]);

  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    // Restore any in-flight op's progress if the user navigated away and back.
    backupStatus()
      .then((s) => {
        if (!mounted.current) return;
        setRunning(s.running);
        setPhase(s.phase);
        setFraction(s.fraction);
        if (s.last_error) setError(s.last_error);
        // Re-offer the "switch to the restored vault" button after the panel was closed and
        // reopened: the backend still holds the staged restore (key + summary) for this session,
        // so we don't make the user redo the whole restore just because the UI unmounted.
        if (s.pending_restore) setRestored(s.pending_restore);
      })
      .catch(() => {});
    const un = onBackupProgress((e) => {
      if (!mounted.current) return;
      if (e.type === "phase") {
        setRunning(true);
        setPhase(e.phase);
        setFraction(e.fraction);
      } else if (e.type === "finished") {
        setRunning(false);
        setPhase(null);
        setFraction(1);
      } else if (e.type === "failed") {
        setRunning(false);
        setPhase(null);
        setError(e.message);
      }
    });
    return () => {
      mounted.current = false;
      un.then((u) => u()).catch(() => {});
    };
  }, []);

  const backupValid = pass.length >= 8 && pass === confirm && !running;
  // A single "any op in flight" gate so connect/disconnect and backup/restore are mutually
  // exclusive in the UI — e.g. Disconnect must be disabled during an upload it would kill.
  const busy = running || protonBusy || gdriveBusy;
  const st = strength(pass);
  const passphraseStored = schedule?.passphrase_stored ?? false;
  const protonConnected = !!(proton?.installed && conn?.connected);
  const gdriveGranted = !!gdrive?.has_write_scope;
  const showStatus = !!schedule && (schedule.frequency !== "off" || !!schedule.last_backup_at);
  // The destinations a scheduled run would push to, for the status summary line.
  const enabledDestinations = [
    schedule?.proton_enabled ? "Proton Drive" : null,
    schedule?.gdrive_enabled ? "Google Drive" : null,
  ].filter(Boolean) as string[];

  async function doRememberPass() {
    if (!backupValid) return;
    setError(null);
    setMessage(null);
    try {
      await setBackupPassphrase(pass);
      await refreshSchedule();
      setMessage(
        "Passphrase remembered on this device — automatic backups can now run unattended.",
      );
    } catch (e) {
      setError(String(e));
    }
  }

  async function doForgetPass() {
    setMessage(null);
    setScheduleSaveError(null);
    setSavingSchedule(true);
    try {
      await forgetBackupPassphrase();
      await refreshSchedule();
    } catch (e) {
      setScheduleSaveError(String(e));
    } finally {
      setSavingSchedule(false);
    }
  }

  async function doBackup() {
    setError(null);
    setMessage(null);
    let dest: string | null;
    try {
      dest = await saveFileDialog({
        defaultPath: "personal-manager-backup.pmbackup",
        filters: [{ name: "PM encrypted backup", extensions: ["pmbackup"] }],
      });
    } catch (e) {
      setError(String(e));
      return;
    }
    if (!dest) return; // cancelled
    try {
      setRunning(true);
      await createLocalBackup(dest, pass);
      // Leave the passphrase in place so the buttons stay live (a common trip-up was them going
      // dead after a save) — you can save again or push the same passphrase to Proton.
      setMessage(
        `Backup saved to ${dest}. Keep this file and your passphrase together — you need both to restore.`,
      );
    } catch (e) {
      setError(String(e));
    } finally {
      setRunning(false);
    }
  }

  async function chooseRestoreFile() {
    setError(null);
    setMessage(null);
    setRestored(null);
    try {
      const picked = await openFileDialog({
        multiple: false,
        filters: [{ name: "PM encrypted backup", extensions: ["pmbackup"] }],
      });
      if (typeof picked === "string") setRestoreSrc(picked);
    } catch (e) {
      setError(String(e));
    }
  }

  async function doRestore() {
    if (!restoreSrc || restorePass.length === 0) return;
    setError(null);
    setMessage(null);
    setRestored(null);
    try {
      setRunning(true);
      const summary = await restoreLocalBackup(restoreSrc, restorePass);
      setRestored(summary);
      setRestorePass("");
    } catch (e) {
      setError(String(e));
    } finally {
      setRunning(false);
    }
  }

  async function doSwitch() {
    if (!restored) return;
    setError(null);
    try {
      setSwitching(true);
      await switchToVault(restored.target_dir);
      // The active vault changed underneath the whole app; reload so every view re-reads it.
      window.location.reload();
    } catch (e) {
      setError(String(e));
      setSwitching(false);
    }
  }

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

  async function doProtonBackup() {
    setError(null);
    setMessage(null);
    try {
      setRunning(true);
      await backupToProton(pass);
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

  async function doGdriveBackup() {
    setError(null);
    setMessage(null);
    try {
      setRunning(true);
      await backupToGdrive(pass);
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

  return (
    <div className="mt-5 border-t border-border pt-4" data-help="settings-backup">
      <label className="block text-sm font-medium text-ink2">Encrypted backup</label>
      <p className="mt-1 text-sm text-ink3">
        A backup is a single encrypted file that holds a complete copy of your whole vault — every
        note, the database, and your settings. You lock it with a passphrase you choose; restoring
        needs that same file and passphrase, here or on any other computer. There&rsquo;s no way to
        recover a backup without its passphrase, so keep it somewhere safe.
      </p>

      {/* Status summary — so reopening the app shows where backups stand at a glance. */}
      {showStatus && schedule && (
        <div className="mt-3 max-w-sm rounded-[var(--radius-sm)] border border-border2 bg-surface p-3">
          <p className="font-mono text-xs uppercase tracking-wide text-ink3">Backup status</p>
          <dl className="mt-1.5 flex flex-col gap-1 text-xs text-ink3">
            <div className="flex justify-between gap-2">
              <dt className="text-ink4">Automatic</dt>
              <dd className="text-right">
                {schedule.frequency === "off"
                  ? "Off"
                  : `${FREQ_LABEL[schedule.frequency]} → ${
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
      )}

      {(running || phase) && (
        <div className="mt-3">
          <IngestProgress
            processed={Math.round(fraction * 100)}
            total={100}
            mode="percent"
            label={phase ? PHASE_LABEL[phase] : "Working"}
          />
          {running && (
            <Button variant="tertiary" className="mt-1" onClick={() => stopBackup()}>
              Stop
            </Button>
          )}
        </div>
      )}

      {error && <p className="mt-2 break-words text-xs text-st-due">{error}</p>}
      {message && <p className="mt-2 break-all text-xs text-st-quick">{message}</p>}

      {/* Shared restore result — surfaced here so either the file OR the Proton restore flow
          shows it prominently, not buried under whichever section started it. */}
      {restored && (
        <div className="mt-3 max-w-sm rounded-[var(--radius-sm)] border border-border2 bg-surface p-3">
          <p className="text-sm text-ink2">Restored a vault, ready to use.</p>
          <p className="mt-1 text-xs text-ink4">
            From a backup made {formatDateTime(restored.created_at)}. It&rsquo;s in a new folder;
            your current vault is still active until you switch.
          </p>
          <div className="mt-2">
            <Button variant="primary" onClick={doSwitch} disabled={switching}>
              {switching ? "Switching…" : "Switch to the restored vault"}
            </Button>
          </div>
        </div>
      )}

      {/* --- 1 · Backup passphrase --- */}
      <div className="mt-5">
        <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
          Backup passphrase
        </label>
        <p className="mt-1 text-xs text-ink4">
          Choose the passphrase that locks your backups. It&rsquo;s a separate secret from your app
          lock, and it&rsquo;s the only thing that can unlock a backup later — there&rsquo;s no
          recovery if you lose it, so store it somewhere safe (a password manager).
        </p>
        <div className="mt-2 flex max-w-sm flex-col gap-2">
          <Input
            type="password"
            autoComplete="new-password"
            placeholder="Backup passphrase"
            value={pass}
            onChange={(e) => setPass(e.currentTarget.value)}
          />
          <Input
            type="password"
            autoComplete="new-password"
            placeholder="Confirm passphrase"
            value={confirm}
            onChange={(e) => setConfirm(e.currentTarget.value)}
          />
          <div className="flex items-center justify-between">
            {st.label ? <span className={`text-xs ${st.tone}`}>{st.label}</span> : <span />}
            {confirm.length > 0 && pass !== confirm && (
              <span className="text-xs text-st-due">Passphrases don&rsquo;t match</span>
            )}
          </div>

          {/* "Remember" stores the KEY (not the data) in the OS keychain — the distinction the
              tab has to make unmistakable, since a passphrase and a .pmbackup are different things. */}
          {passphraseStored ? (
            <div className="flex items-center justify-between gap-2 text-xs">
              <span className="text-st-quick">Passphrase remembered on this device</span>
              <Button variant="tertiary" onClick={doForgetPass} disabled={savingSchedule || busy}>
                Forget
              </Button>
            </div>
          ) : (
            <div className="flex flex-col gap-1">
              <div>
                <Button variant="secondary" onClick={doRememberPass} disabled={!backupValid}>
                  Remember on this device
                </Button>
              </div>
              <p className="text-xs text-ink4">
                Optional. Stores only the passphrase in your OS keychain — never your data — so
                automatic backups can run without asking. Required to turn on a schedule below.
              </p>
            </div>
          )}
        </div>
      </div>

      {/* --- 2 · Save a backup now --- */}
      <div className="mt-6">
        <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
          Save a backup now
        </label>
        <p className="mt-1 text-xs text-ink4">
          Packs your whole vault into one encrypted <span className="font-mono">.pmbackup</span>{" "}
          file and locks it with the passphrase above. That file{" "}
          <span className="font-medium">is your data</span> — compressed and encrypted — not your
          passphrase; you need both to restore.
        </p>
        <div className="mt-2 flex max-w-sm flex-col gap-2">
          <div className="flex flex-wrap gap-2">
            <Button variant="primary" onClick={doBackup} disabled={!backupValid}>
              Save to this computer…
            </Button>
            {protonConnected && (
              <Button variant="secondary" onClick={doProtonBackup} disabled={!backupValid || busy}>
                Save to Proton Drive…
              </Button>
            )}
            {gdriveGranted && (
              <Button variant="secondary" onClick={doGdriveBackup} disabled={!backupValid || busy}>
                Save to Google Drive…
              </Button>
            )}
          </div>
          {!backupValid && !running && (
            <p className="text-xs text-ink4">
              Enter a matching passphrase (8+ characters) above to enable these buttons.
            </p>
          )}
          {backupValid && !protonConnected && !gdriveGranted && (
            <p className="text-xs text-ink4">
              Connect Proton Drive or Google Drive below to also save backups off-machine.
            </p>
          )}
        </div>
      </div>

      {/* --- Restore a backup --- */}
      <div className="mt-6">
        <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
          Restore a backup
        </label>
        <p className="mt-1 text-xs text-ink4">
          Have a <span className="font-mono">.pmbackup</span> file? It&rsquo;s your whole vault,
          compressed and encrypted. Choose it and enter its passphrase — restore unpacks it into a
          new folder and verifies it first, so your current vault is untouched until you switch to
          the restored one.
        </p>
        <div className="mt-2 flex max-w-sm flex-col gap-2">
          <div className="flex items-center gap-2">
            <Button variant="secondary" onClick={chooseRestoreFile} disabled={running}>
              Choose backup file…
            </Button>
            {restoreSrc && (
              <span className="min-w-0 truncate text-xs text-ink4" title={restoreSrc}>
                {restoreSrc}
              </span>
            )}
          </div>
          {restoreSrc && (
            <>
              <Input
                type="password"
                autoComplete="off"
                placeholder="Backup passphrase"
                value={restorePass}
                onChange={(e) => setRestorePass(e.currentTarget.value)}
              />
              <div>
                <Button
                  variant="primary"
                  onClick={doRestore}
                  disabled={running || restorePass.length === 0}
                >
                  Restore…
                </Button>
              </div>
            </>
          )}
        </div>
      </div>

      {/* --- Proton Drive (off-machine destination + automatic backups) --- */}
      <div className="mt-6">
        <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
          Proton Drive
        </label>
        <p className="mt-1 text-xs text-ink4">
          Keep your encrypted backups off-machine on your own Proton Drive — end-to-end-encrypted
          cold storage. PM uses Proton&rsquo;s official command-line tool and never sees your Proton
          login.
        </p>

        {proton === null ? (
          <p className="mt-2 text-xs text-ink4">Checking for the Proton Drive CLI&hellip;</p>
        ) : !proton.installed ? (
          <div className="mt-2 flex max-w-sm flex-col gap-2">
            <p className="text-xs text-ink4">
              The Proton Drive CLI isn&rsquo;t installed. Install the official build to back up here
              — PM detects it automatically.
            </p>
            <div>
              <Button
                variant="secondary"
                onClick={() => void openUrl(proton.install_url).catch(() => {})}
              >
                Get the Proton Drive CLI&hellip;
              </Button>
            </div>
          </div>
        ) : conn === null ? (
          <p className="mt-2 text-xs text-ink4">Checking your Proton Drive connection&hellip;</p>
        ) : !conn.connected ? (
          <div className="mt-2 flex max-w-sm flex-col gap-2">
            <p className="text-xs text-ink4">
              Sign in to your Proton account to push and pull backups. PM opens your browser; your
              login is handled entirely by Proton.
            </p>
            <div>
              <Button variant="secondary" onClick={doProtonConnect} disabled={busy}>
                {protonBusy ? "Connecting… finish in your browser" : "Connect Proton Drive"}
              </Button>
            </div>
            {conn.error && <p className="break-words text-xs text-st-due">{conn.error}</p>}
          </div>
        ) : (
          <div className="mt-2 flex max-w-sm flex-col gap-3">
            <div className="flex items-center justify-between gap-2 rounded-[var(--radius-sm)] border border-border2 bg-surface p-3">
              <div className="min-w-0">
                <p className="text-sm text-st-quick">Connected</p>
                {conn.account && (
                  <p className="truncate text-xs text-ink4" title={conn.account}>
                    {conn.account}
                  </p>
                )}
              </div>
              <Button variant="tertiary" onClick={doProtonDisconnect} disabled={busy}>
                Disconnect
              </Button>
            </div>
            <p className="text-xs text-ink4">
              Enter a passphrase under &ldquo;Backup passphrase&rdquo; above, then choose{" "}
              <span className="font-medium">Save to Proton Drive</span> — or set up automatic
              backups below.
            </p>

            <div>
              <p className="font-mono text-xs uppercase tracking-wide text-ink3">On Proton Drive</p>
              {protonListError ? (
                <div className="mt-1 flex items-center gap-2">
                  <span className="text-xs text-st-due">Couldn&rsquo;t load your backups.</span>
                  <Button variant="tertiary" onClick={() => void refreshProton()} disabled={busy}>
                    Retry
                  </Button>
                </div>
              ) : protonBackups === null ? (
                <p className="mt-1 text-xs text-ink4">Loading&hellip;</p>
              ) : protonBackups.length === 0 ? (
                <p className="mt-1 text-xs text-ink4">No backups yet.</p>
              ) : (
                <ul className="mt-1 flex flex-col gap-1">
                  {protonBackups.map((b) => (
                    <li key={b.name} className="flex items-center justify-between gap-2 text-xs">
                      <span className="min-w-0 truncate text-ink3" title={b.name}>
                        {b.name}
                      </span>
                      <Button
                        variant="tertiary"
                        onClick={() => {
                          setProtonRestoreName(b.name);
                          setProtonRestorePass("");
                          setRestored(null);
                        }}
                        disabled={busy}
                      >
                        Restore
                      </Button>
                    </li>
                  ))}
                </ul>
              )}
            </div>

            {protonRestoreName && (
              <div className="flex flex-col gap-2 rounded-[var(--radius-sm)] border border-border2 bg-surface p-3">
                <p className="text-xs text-ink4">
                  Restore <span className="break-all font-medium">{protonRestoreName}</span> — enter
                  its passphrase. It unpacks into a new folder; your current vault is untouched
                  until you switch.
                </p>
                <Input
                  type="password"
                  autoComplete="off"
                  placeholder="Backup passphrase"
                  value={protonRestorePass}
                  onChange={(e) => setProtonRestorePass(e.currentTarget.value)}
                />
                <div className="flex gap-2">
                  <Button
                    variant="primary"
                    onClick={doProtonRestore}
                    disabled={busy || protonRestorePass.length === 0}
                  >
                    Restore&hellip;
                  </Button>
                  <Button
                    variant="tertiary"
                    onClick={() => {
                      setProtonRestoreName(null);
                      setProtonRestorePass("");
                    }}
                  >
                    Cancel
                  </Button>
                </div>
              </div>
            )}
          </div>
        )}
      </div>

      {/* --- Google Drive (off-machine destination via the Drive API — already connected, so this
          only grants the one extra write permission and reflects status; no install flow) --- */}
      <div className="mt-6">
        <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
          Google Drive
        </label>
        <p className="mt-1 text-xs text-ink4">
          Keep your encrypted backups on your own Google Drive. They&rsquo;re already encrypted
          before they leave your computer; PM only ever touches its own &ldquo;Personal Manager
          Backups&rdquo; folder (the <span className="font-mono">drive.file</span> permission),
          never the rest of your Drive.
        </p>

        {gdrive === null ? (
          <p className="mt-2 text-xs text-ink4">Checking your Google Drive connection&hellip;</p>
        ) : !gdriveGranted ? (
          <div className="mt-2 flex max-w-sm flex-col gap-2">
            <p className="text-xs text-ink4">
              Grant PM permission to save backups to your Google Drive — a one-time approval in your
              browser. It only lets PM manage the backups it creates.
            </p>
            {gdrive.accounts.length > 0 && (
              <label className="flex items-center justify-between gap-2 text-xs text-ink3">
                <span>Account</span>
                <select
                  className="min-w-0 rounded-[var(--radius-sm)] border border-border bg-surface px-2 py-1 text-ink2"
                  value={gdriveAccountChoice || gdrive.accounts[0]?.email || ""}
                  onChange={(e) => setGdriveAccountChoice(e.currentTarget.value)}
                  disabled={busy}
                >
                  {gdrive.accounts.map((a) => (
                    <option key={a.email} value={a.email}>
                      {a.email}
                    </option>
                  ))}
                </select>
              </label>
            )}
            <div>
              <Button
                variant="secondary"
                onClick={() =>
                  void doGdriveConnect(
                    gdrive.accounts.length > 0
                      ? gdriveAccountChoice || gdrive.accounts[0]?.email
                      : undefined,
                  )
                }
                disabled={busy}
              >
                {gdriveBusy ? "Granting… finish in your browser" : "Grant backup access…"}
              </Button>
            </div>
          </div>
        ) : (
          <div className="mt-2 flex max-w-sm flex-col gap-3">
            <div className="flex items-center justify-between gap-2 rounded-[var(--radius-sm)] border border-border2 bg-surface p-3">
              <div className="min-w-0">
                <p className="text-sm text-st-quick">Connected</p>
                {gdrive.account && (
                  <p className="truncate text-xs text-ink4" title={gdrive.account}>
                    {gdrive.account}
                  </p>
                )}
              </div>
              <Button variant="tertiary" onClick={doGdriveDisconnect} disabled={busy}>
                Disconnect
              </Button>
            </div>
            <p className="text-xs text-ink4">
              Enter a passphrase under &ldquo;Backup passphrase&rdquo; above, then choose{" "}
              <span className="font-medium">Save to Google Drive</span> — or set up automatic
              backups below.
            </p>

            <div>
              <p className="font-mono text-xs uppercase tracking-wide text-ink3">On Google Drive</p>
              {gdriveListError ? (
                <div className="mt-1 flex items-center gap-2">
                  <span className="text-xs text-st-due">Couldn&rsquo;t load your backups.</span>
                  <Button variant="tertiary" onClick={() => void refreshGdrive()} disabled={busy}>
                    Retry
                  </Button>
                </div>
              ) : gdriveBackups === null ? (
                <p className="mt-1 text-xs text-ink4">Loading&hellip;</p>
              ) : gdriveBackups.length === 0 ? (
                <p className="mt-1 text-xs text-ink4">No backups yet.</p>
              ) : (
                <ul className="mt-1 flex flex-col gap-1">
                  {gdriveBackups.map((b) => (
                    <li key={b.name} className="flex items-center justify-between gap-2 text-xs">
                      <span className="min-w-0 truncate text-ink3" title={b.name}>
                        {b.name}
                      </span>
                      <Button
                        variant="tertiary"
                        onClick={() => {
                          setGdriveRestoreName(b.name);
                          setGdriveRestorePass("");
                          setRestored(null);
                        }}
                        disabled={busy}
                      >
                        Restore
                      </Button>
                    </li>
                  ))}
                </ul>
              )}
            </div>

            {gdriveRestoreName && (
              <div className="flex flex-col gap-2 rounded-[var(--radius-sm)] border border-border2 bg-surface p-3">
                <p className="text-xs text-ink4">
                  Restore <span className="break-all font-medium">{gdriveRestoreName}</span> — enter
                  its passphrase. It unpacks into a new folder; your current vault is untouched
                  until you switch.
                </p>
                <Input
                  type="password"
                  autoComplete="off"
                  placeholder="Backup passphrase"
                  value={gdriveRestorePass}
                  onChange={(e) => setGdriveRestorePass(e.currentTarget.value)}
                />
                <div className="flex gap-2">
                  <Button
                    variant="primary"
                    onClick={doGdriveRestore}
                    disabled={busy || gdriveRestorePass.length === 0}
                  >
                    Restore&hellip;
                  </Button>
                  <Button
                    variant="tertiary"
                    onClick={() => {
                      setGdriveRestoreName(null);
                      setGdriveRestorePass("");
                    }}
                  >
                    Cancel
                  </Button>
                </div>
              </div>
            )}
          </div>
        )}
      </div>

      {/* --- Automatic backups — one schedule fans out to every destination you turn on --- */}
      <div className="mt-6 border-t border-border pt-4">
        <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
          Automatic backups
        </label>
        <p className="mt-1 text-xs text-ink4">
          PM backs up your current vault on a schedule, using the passphrase you remembered above,
          to whichever destinations you turn on here.
        </p>
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
              <select
                className="rounded-[var(--radius-sm)] border border-border bg-surface px-2 py-1 text-ink2"
                value={freqDraft}
                onChange={(e) => setFreqDraft(e.currentTarget.value as BackupSchedule["frequency"])}
                disabled={savingSchedule || busy}
              >
                <option value="off">Off</option>
                <option value="daily">Daily</option>
                <option value="weekly">Weekly</option>
                <option value="monthly">Monthly</option>
              </select>
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
                Remember your backup passphrase above (under &ldquo;Backup passphrase&rdquo;) to
                turn on automatic backups.
              </p>
            )}

            {freqDraft !== "off" && passphraseStored && enabledDestinations.length === 0 && (
              <p className="text-xs text-st-due">
                Turn on at least one destination above for scheduled backups to run.
              </p>
            )}

            <div>
              <Button
                variant="secondary"
                onClick={doSaveSchedule}
                disabled={savingSchedule || busy || (freqDraft !== "off" && !passphraseStored)}
              >
                {savingSchedule ? "Saving…" : "Save schedule"}
              </Button>
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
      </div>

      <div className="mt-4">
        <Collapsible title="How encrypted backup works" defaultOpen={showPower}>
          <div className="pt-2 text-xs leading-relaxed text-ink4">
            <p>
              A backup bundles your encrypted database, the Markdown vault, and the vault metadata,
              compresses it with zstd, then encrypts the whole archive with a key stretched from
              your passphrase (Argon2id). The archive is self-contained: it can be restored on a
              different computer with only the passphrase.
            </p>
            <p className="mt-1">
              This is different from <span className="font-medium">Export all data</span> (a plain
              .zip that only opens on this machine) and from{" "}
              <span className="font-medium">Export plaintext Markdown</span> (a readable,
              unencrypted copy of your notes). Keep your passphrase safe — a backup can&rsquo;t be
              recovered without it.
            </p>
          </div>
        </Collapsible>
      </div>
    </div>
  );
}
