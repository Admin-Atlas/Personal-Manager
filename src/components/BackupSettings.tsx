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
  backupArchivePrefix,
  backupGdriveConnect,
  backupGdriveDisconnect,
  backupGdriveStatus,
  backupNow,
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
  pruneOwnBackups,
  restoreFromGdrive,
  restoreFromProton,
  restoreLocalBackup,
  setBackupDestinations,
  setBackupPassphrase,
  setBackupSchedule,
  setProtonCliPath,
  stopBackup,
  switchToVault,
} from "../lib/ipc";
import type {
  BackupEntry,
  BackupPhase,
  BackupSchedule,
  GdriveBackupStatus,
  PassphraseScore,
  ProtonCliStatus,
  ProtonConnStatus,
  RestoreSummary,
} from "../lib/types";
import { formatDateTime } from "../lib/format";
import {
  BACKUP_FREQUENCY_LABEL,
  describeFailures,
  describeForgetConsequences,
  isOpaquePhase,
} from "../lib/backup";
import { readReconcileDismissed, writeReconcileDismissed } from "../lib/backupPrefs";
import { useRegisterPending } from "../lib/settingsPending";
import { Button, ConfirmDialog, Input, SectionInfo, SectionLabel, Select } from "./ui";
import { PassphraseStrengthMeter } from "./PassphraseStrengthMeter";
import { IngestProgress } from "./IngestProgress";

const PHASE_LABEL: Record<BackupPhase, string> = {
  snapshot: "Preparing a snapshot",
  pack: "Compressing & encrypting",
  upload: "Uploading",
  download: "Downloading",
  restore: "Decrypting & unpacking",
  validate: "Verifying",
};

export function BackupSettings() {
  // Live progress (from the global backup://progress event + a status snapshot on mount).
  const [running, setRunning] = useState(false);
  const [phase, setPhase] = useState<BackupPhase | null>(null);
  const [fraction, setFraction] = useState(0);
  // Epoch ms the running backup/restore began, restored from the backend snapshot so leaving and
  // reopening this panel mid-op doesn't restart the elapsed timer. The backend stamps it
  // edge-triggered on idle -> running, so it survives every phase transition.
  const [startedAt, setStartedAt] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  // Non-blocking "backed up, but some destinations failed" banner (F-22): distinct from `error`
  // (the whole op failed) and `message` (clean success). Set from a finished run's report.
  const [warning, setWarning] = useState<string | null>(null);

  // The passphrase that locks every backup (manual saves read it directly; "Remember on this
  // device" additionally stows it in the keychain for unattended scheduled runs).
  const [pass, setPass] = useState("");
  const [confirm, setConfirm] = useState("");
  const [passScore, setPassScore] = useState<PassphraseScore | null>(null);

  // Restore form.
  const [restoreSrc, setRestoreSrc] = useState<string | null>(null);
  const [restorePass, setRestorePass] = useState("");
  const [restored, setRestored] = useState<RestoreSummary | null>(null);
  const [switching, setSwitching] = useState(false);
  // When a restored vault was passphrase-protected ("shareable"), the user chooses on this machine:
  // make it private (default — none of the sharing setup travels in a backup), or keep the passphrase.
  const [restoreAsPrivate, setRestoreAsPrivate] = useState(true);

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

  // Outcome of the banner's "Delete oldest" action, reported per destination and rendered IN the
  // banner. The action used to discard its count and route failures to the top-level `error` sink,
  // which renders hundreds of lines above this inside a different panel — scrolled off-screen on any
  // normal window. Success said nothing and failure said nothing visible, so the click read as
  // "it did nothing" either way.
  const [pruneNote, setPruneNote] = useState<{ proton: string | null; gdrive: string | null }>({
    proton: null,
    gdrive: null,
  });

  // The three one-way doors on this panel, each behind its own confirmation. Disconnect shares one
  // piece of state across both destinations because only one of the two dialogs can ever be open;
  // if that ever stops being true, split it rather than widening the union.
  const [confirmForget, setConfirmForget] = useState(false);
  const [confirmDisconnect, setConfirmDisconnect] = useState<"proton" | "gdrive" | null>(null);

  // This vault's archive-name prefix, so we can count only THIS vault's archives at a shared
  // destination for the "you have more backups than keep-last-N" reconciliation banner. Loaded once.
  const [archivePrefix, setArchivePrefix] = useState<string | null>(null);
  // Per-destination dismissal of that banner (persisted, keyed by destination + account).
  const [reconcileDismissed, setReconcileDismissed] = useState<{
    proton: boolean;
    gdrive: boolean;
  }>({ proton: false, gdrive: false });

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
  // Re-probe when the window regains focus while the CLI still isn't detected, so installing it
  // while PM is open is picked up without a restart. Gated to the not-found case so it never
  // re-spawns the CLI (a `proton_status` call) on every focus once it's already located.
  useEffect(() => {
    if (proton?.installed) return;
    const onFocus = () => void refreshProton();
    window.addEventListener("focus", onFocus);
    return () => window.removeEventListener("focus", onFocus);
  }, [proton?.installed, refreshProton]);
  useEffect(() => {
    void refreshGdrive();
  }, [refreshGdrive]);
  useEffect(() => {
    void refreshSchedule();
  }, [refreshSchedule]);
  // The vault's archive prefix is stable for the session — fetch it once.
  useEffect(() => {
    backupArchivePrefix()
      .then(setArchivePrefix)
      .catch(() => setArchivePrefix(null));
  }, []);
  // Re-read the banner's dismissal whenever the connected account changes (a different account is a
  // different backup location, so a prior dismissal shouldn't carry over).
  useEffect(() => {
    setReconcileDismissed({
      proton: readReconcileDismissed("proton", conn?.account ?? null),
      gdrive: readReconcileDismissed("gdrive", schedule?.gdrive_account ?? null),
    });
  }, [conn?.account, schedule?.gdrive_account]);

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
        setStartedAt(s.started_at_ms);
        if (s.last_error) setError(s.last_error);
        // Re-surface a partial-failure banner from the last finished run (F-22), so navigating
        // away and back doesn't lose "backed up, but Google Drive failed".
        if (s.last_report) setWarning(describeFailures(s.last_report.failed_destinations));
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
        // Only seeds when we have nothing: every phase transition emits one of these, so assigning
        // unconditionally would restart the timer at snapshot -> pack -> upload. The authoritative
        // stamp comes from the snapshot fetch above; this just covers a mount that beat it.
        setStartedAt((prev) => prev ?? Date.now());
        setWarning(null); // a fresh run started — drop any stale partial-failure banner
      } else if (e.type === "finished") {
        setRunning(false);
        setPhase(null);
        setFraction(1);
        setStartedAt(null);
        // F-22: some destinations may have failed while others succeeded (a partial success the
        // backend still reports as "finished"). Surface them non-blockingly; null clears cleanly.
        setWarning(describeFailures(e.report.failed_destinations));
      } else if (e.type === "failed") {
        setRunning(false);
        setPhase(null);
        setStartedAt(null);
        setError(e.message);
      }
    });
    return () => {
      mounted.current = false;
      un.then((u) => u()).catch(() => {});
    };
  }, []);

  // The backend floor (validate_passphrase_strength) is the real gate; here we block a KNOWN-weak
  // passphrase (immediate feedback) while a scoring hiccup (null) never soft-locks the buttons.
  const backupValid =
    pass.length > 0 && pass === confirm && !running && passScore?.acceptable !== false;
  // A single "any op in flight" gate so connect/disconnect and backup/restore are mutually
  // exclusive in the UI — e.g. Disconnect must be disabled during an upload it would kill.
  const busy = running || protonBusy || gdriveBusy;
  const passphraseStored = schedule?.passphrase_stored ?? false;
  // The cadence half of what "Forget" costs — null when there is no schedule to lose, so the
  // confirmation never warns about losing something the user hasn't got.
  const forgetConsequence = describeForgetConsequences(schedule?.frequency ?? "off");
  const protonConnected = !!(proton?.installed && conn?.connected);
  const gdriveGranted = !!gdrive?.has_write_scope;
  // Whether the account set up for BACKUP is also connected as a read-only Drive source. It decides
  // what disconnecting costs: the backend only deletes the keychain token when the account is not
  // also a connector (`backup_gdrive_disconnect`), so one case keeps working and the other needs a
  // fresh `drive.file` grant. Case-insensitive, matching the backend's `eq_ignore_ascii_case`.
  const gdriveAccount = gdrive?.account ?? null;
  const gdriveAlsoConnector =
    gdriveAccount !== null &&
    (gdrive?.accounts ?? []).some((a) => a.email.toLowerCase() === gdriveAccount.toLowerCase());
  const showStatus = !!schedule && (schedule.frequency !== "off" || !!schedule.last_backup_at);
  // The destinations a scheduled run would push to, for the status summary line.
  const enabledDestinations = [
    schedule?.proton_enabled ? "Proton Drive" : null,
    schedule?.gdrive_enabled ? "Google Drive" : null,
  ].filter(Boolean) as string[];

  // This vault's own archive count at each destination (the listing includes every vault's archives
  // sharing the account/folder, so filter by our prefix). `null` until both the prefix and the
  // listing have loaded. The reconciliation banner fires when a destination holds more than keep-N.
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
  const protonOwnCount =
    archivePrefix && protonBackups
      ? protonBackups.filter((b) => b.name.startsWith(archivePrefix)).length
      : null;
  const gdriveOwnCount =
    archivePrefix && gdriveBackups
      ? gdriveBackups.filter((b) => b.name.startsWith(archivePrefix)).length
      : null;
  const protonOverLimit = protonOwnCount !== null && keepN !== null && protonOwnCount > keepN;
  const gdriveOverLimit = gdriveOwnCount !== null && keepN !== null && gdriveOwnCount > keepN;

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

  async function doBackup() {
    setError(null);
    setMessage(null);
    setWarning(null); // a fresh backup supersedes any prior run's partial-failure notice
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
      // A device-mode restore is already private; only a passphrase restore honours the choice.
      const makePrivate = restored.key_mode === "passphrase" ? restoreAsPrivate : true;
      await switchToVault(restored.target_dir, makePrivate);
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

  // "Back up now" from a connected panel: uses the STORED passphrase and prunes to keep-last-N (a
  // scheduled-style run), so it only appears once a passphrase is remembered. Distinct from the
  // typed-passphrase "Save to …" buttons in the section above, which never prune.
  async function doBackupNow(kind: "proton" | "gdrive") {
    setError(null);
    setMessage(null);
    try {
      setRunning(true);
      await backupNow(kind);
      setMessage(kind === "proton" ? "Backed up to Proton Drive." : "Backed up to Google Drive.");
      if (kind === "proton") await refreshProton();
      else await refreshGdrive();
      await refreshSchedule();
    } catch (e) {
      setError(String(e));
    } finally {
      setRunning(false);
    }
  }

  // Reconciliation banner action: raise keep-last-N to the number already at the destination, so the
  // next backup rolls the oldest off instead of the setting quietly capping below what's stored.
  async function doRaiseKeepN(kind: "proton" | "gdrive") {
    const present = kind === "proton" ? protonOwnCount : gdriveOwnCount;
    if (present == null) return;
    setScheduleSaveError(null);
    try {
      // Keep the current cadence; only lift the retention. ("off" needs no passphrase, so this works
      // whether or not automatic backups are on.)
      await setBackupSchedule(schedule?.frequency ?? "off", present);
      await refreshSchedule();
    } catch (e) {
      setScheduleSaveError(String(e));
    }
  }

  // Reconciliation banner action: trim this vault's archives at the destination to keep-last-N now
  // (recoverable — Proton/Drive trash).
  //
  // `skipped` is not a failure: Google Drive only lets PM modify files its own sign-in created, so
  // archives uploaded under an earlier grant stay listed but refuse to be trashed. Saying so beats
  // "Moved 0" over a banner that still shows ten.
  function prunedNote(trashed: number, skipped: number): string {
    const moved =
      trashed > 0 ? `Moved ${trashed} older backup${trashed === 1 ? "" : "s"} to the trash. ` : "";
    if (skipped === 0) {
      return trashed > 0
        ? moved.trim()
        : "Nothing to trim — none of this vault's archives were over the limit.";
    }
    return `${moved}PM can only remove backups it uploaded with the current Google sign-in, so ${skipped} older archive${
      skipped === 1 ? "" : "s"
    } stayed put. Delete ${skipped === 1 ? "it" : "them"} in Google Drive to free the space.`;
  }

  async function doPruneOldest(kind: "proton" | "gdrive") {
    setPruneNote((prev) => ({ ...prev, [kind]: null }));
    try {
      if (kind === "proton") setProtonBusy(true);
      else setGdriveBusy(true);
      const { trashed, skipped } = await pruneOwnBackups(kind);
      setPruneNote((prev) => ({ ...prev, [kind]: prunedNote(trashed, skipped) }));
    } catch (e) {
      setPruneNote((prev) => ({ ...prev, [kind]: String(e) }));
    } finally {
      // Refresh even when the trim threw or was refused: a partial pass still moved archives, and
      // leaving the banner showing a stale count reads as "the button did nothing".
      if (kind === "proton") await refreshProton().catch(() => {});
      else await refreshGdrive().catch(() => {});
      if (kind === "proton") setProtonBusy(false);
      else setGdriveBusy(false);
    }
  }

  function doDismissReconcile(kind: "proton" | "gdrive") {
    const account =
      kind === "proton" ? (conn?.account ?? null) : (schedule?.gdrive_account ?? null);
    writeReconcileDismissed(kind, account);
    setReconcileDismissed((prev) => ({ ...prev, [kind]: true }));
  }

  // The banner shown at the top of a connected panel when the destination holds more of THIS vault's
  // archives than keep-last-N — offering to keep them all (raise N) or trim to N now.
  function reconcileBanner(kind: "proton" | "gdrive") {
    const present = kind === "proton" ? protonOwnCount : gdriveOwnCount;
    const over = kind === "proton" ? protonOverLimit : gdriveOverLimit;
    const note = pruneNote[kind];
    const destBusy = kind === "proton" ? protonBusy : gdriveBusy;
    if (!over || reconcileDismissed[kind] || present == null || keepN == null) {
      // A successful trim clears `over`, which would take the banner — and its outcome line — away
      // in the same render. Keep the note on screen after the banner it belongs to has gone.
      return note ? <p className="text-sm text-ink3">{note}</p> : null;
    }
    return (
      <div
        className="flex flex-col gap-2 rounded-[var(--radius)] border px-3 py-2.5 text-sm text-ink3"
        style={{
          borderColor: "color-mix(in oklab, var(--st-due) 35%, transparent)",
          background: "color-mix(in oklab, var(--st-due) 12%, transparent)",
        }}
      >
        <p>
          This destination holds <span className="font-medium">{present}</span> backups of this
          vault — more than your keep-last-{keepN} limit. Older backups aren&rsquo;t trimmed
          automatically until you reconcile this.
        </p>
        <div className="flex flex-wrap gap-2">
          <Button variant="secondary" onClick={() => void doRaiseKeepN(kind)} disabled={busy}>
            Keep all {present}
          </Button>
          <Button
            variant="tertiary"
            onClick={() => void doPruneOldest(kind)}
            disabled={busy || destBusy}
          >
            {destBusy ? "Trimming…" : `Delete oldest, keep ${keepN}`}
          </Button>
          <Button variant="tertiary" onClick={() => doDismissReconcile(kind)} disabled={busy}>
            Dismiss
          </Button>
        </div>
        {note && <p className="text-sm">{note}</p>}
      </div>
    );
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
      <h2 className="block text-sm font-medium text-ink2">Encrypted backup</h2>
      {/* The no-recovery sentence stays inline and unfoldable — the one line whose absence
          costs a vault. The description of what a backup *is* folds beneath it. */}
      <p className="mt-1 text-sm text-ink3">
        There&rsquo;s no way to recover a backup without its passphrase, so keep it somewhere safe.
      </p>
      <SectionInfo title="What is a backup?">
        <p>
          A backup is a single encrypted file that holds a complete copy of your whole vault — every
          note, the database, and your settings. You lock it with a passphrase you choose; restoring
          needs that same file and passphrase, here or on any other computer.
        </p>
      </SectionInfo>

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
      )}

      {(running || phase) && (
        <div className="mt-3">
          <IngestProgress
            processed={Math.round(fraction * 100)}
            // F-45: the upload/download fraction is a coarse per-destination fan-out step (0 then 1
            // for a single target), not real byte-progress — so shimmer (total=null) instead of a
            // bar frozen at 0% through a minutes-long transfer.
            total={isOpaquePhase(phase) ? null : 100}
            startedAt={startedAt ?? undefined}
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
      {/* F-22: a partial success (some destinations failed, at least one succeeded) — a non-blocking
          notice in the shared warning-box idiom, kept separate from `error` (whole op failed). */}
      {warning && (
        <div className="mt-2 break-words rounded-[var(--radius)] border px-3 py-2 text-sm text-st-due">
          {warning}
        </div>
      )}
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
          {restored.key_mode === "passphrase" && (
            <fieldset className="mt-2 flex flex-col gap-1" disabled={switching}>
              <legend className="text-xs text-ink4">
                This backup was passphrase-protected for sharing. On this device:
              </legend>
              <label className="flex cursor-pointer items-start gap-2 text-xs text-ink3">
                <input
                  type="radio"
                  name="restore-privacy"
                  className="mt-0.5"
                  checked={restoreAsPrivate}
                  onChange={() => setRestoreAsPrivate(true)}
                />
                <span>
                  <span className="text-ink2">Make it private to this device</span> — recommended;
                  your notes are re-encrypted with a device key and open without a passphrase.
                </span>
              </label>
              <label className="flex cursor-pointer items-start gap-2 text-xs text-ink3">
                <input
                  type="radio"
                  name="restore-privacy"
                  className="mt-0.5"
                  checked={!restoreAsPrivate}
                  onChange={() => setRestoreAsPrivate(false)}
                />
                <span>
                  <span className="text-ink2">Keep it passphrase-protected</span> — notes stay
                  encrypted at rest; you can share the vault again later.
                </span>
              </label>
            </fieldset>
          )}
          <div className="mt-2">
            <Button variant="primary" onClick={doSwitch} disabled={switching}>
              {switching ? "Switching…" : "Switch to the restored vault"}
            </Button>
          </div>
        </div>
      )}

      {/* --- 1 · Backup passphrase --- */}
      <div className="mt-5">
        <SectionLabel>Backup passphrase</SectionLabel>
        <p className="mt-1 text-xs text-ink4">
          This passphrase is the only thing that can unlock a backup later — there&rsquo;s no
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
          <PassphraseStrengthMeter passphrase={pass} onScored={setPassScore} />
          {confirm.length > 0 && pass !== confirm && (
            <span className="text-xs text-st-due">Passphrases don&rsquo;t match</span>
          )}

          {/* "Remember" stores the KEY (not the data) in the OS keychain — the distinction the
              tab has to make unmistakable, since a passphrase and a .pmbackup are different things. */}
          {passphraseStored ? (
            <div className="flex items-center justify-between gap-2 text-xs">
              <span className="text-st-quick">Passphrase remembered on this device</span>
              <Button
                variant="tertiary"
                onClick={() => setConfirmForget(true)}
                disabled={savingSchedule || busy}
              >
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
                Optional — but required to turn on a schedule below.
              </p>
            </div>
          )}
        </div>
        <SectionInfo title="How the backup passphrase works">
          <p>
            Choose the passphrase that locks your backups. It&rsquo;s a separate secret from your
            app lock.
          </p>
          <p>
            <span className="font-medium">Remember on this device</span> stores only the passphrase
            in your OS keychain — never your data — so automatic backups can run without asking.
          </p>
        </SectionInfo>
      </div>

      {/* --- 2 · Save a backup now --- */}
      <div className="mt-6">
        <SectionLabel>Save a backup now</SectionLabel>
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
              Enter a matching, strong passphrase (10+ characters) above to enable these buttons.
            </p>
          )}
          {backupValid && !protonConnected && !gdriveGranted && (
            <p className="text-xs text-ink4">
              Connect Proton Drive or Google Drive below to also save backups off-machine.
            </p>
          )}
        </div>
        <SectionInfo title="What gets saved?">
          <p>
            Packs your whole vault into one encrypted <span className="font-mono">.pmbackup</span>{" "}
            file and locks it with the passphrase above. That file{" "}
            <span className="font-medium">is your data</span> — compressed and encrypted — not your
            passphrase; you need both to restore.
          </p>
        </SectionInfo>
      </div>

      {/* --- Restore a backup --- */}
      <div className="mt-6">
        <SectionLabel>Restore a backup</SectionLabel>
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
        <SectionInfo title="How restoring works">
          <p>
            Have a <span className="font-mono">.pmbackup</span> file? It&rsquo;s your whole vault,
            compressed and encrypted. Choose it and enter its passphrase — restore unpacks it into a
            new folder and verifies it first, so your current vault is untouched until you switch to
            the restored one.
          </p>
        </SectionInfo>
      </div>

      {/* --- Proton Drive (off-machine destination + automatic backups) --- */}
      <div className="mt-6">
        <SectionLabel>Proton Drive</SectionLabel>
        {proton === null ? (
          <p className="mt-2 text-xs text-ink4">Checking for the Proton Drive CLI&hellip;</p>
        ) : !proton.installed ? (
          <div className="mt-2 flex max-w-sm flex-col gap-2">
            <p className="text-xs text-ink4">
              The Proton Drive CLI isn&rsquo;t installed. Download the official build — it&rsquo;s a
              single program you can keep anywhere. If it&rsquo;s in your Downloads or on your PATH,
              just <span className="text-ink3">Check again</span>; otherwise point PM straight at
              it.
            </p>
            <div className="flex flex-wrap gap-2">
              <Button
                variant="secondary"
                onClick={() => void openUrl(proton.install_url).catch(() => {})}
              >
                Get the Proton Drive CLI&hellip;
              </Button>
              <Button variant="secondary" onClick={() => void locateCli()} disabled={locating}>
                {locating ? "Locating…" : "Locate manually…"}
              </Button>
              <Button variant="tertiary" onClick={() => void refreshProton()} disabled={locating}>
                Check again
              </Button>
            </div>
            {locateError && <p className="break-words text-xs text-st-due">{locateError}</p>}
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
            {reconcileBanner("proton")}
            <div className="flex items-center justify-between gap-2 rounded-[var(--radius-sm)] border border-border2 bg-surface p-3">
              <div className="min-w-0">
                <p className="text-sm text-st-quick">Connected</p>
                {conn.account && (
                  <p className="truncate text-xs text-ink4" title={conn.account}>
                    {conn.account}
                  </p>
                )}
              </div>
              <Button
                variant="tertiary"
                onClick={() => setConfirmDisconnect("proton")}
                disabled={busy}
              >
                Disconnect
              </Button>
            </div>
            {passphraseStored ? (
              <div className="flex items-center justify-between gap-2">
                <p className="text-xs text-ink4">
                  Backs up with your remembered passphrase and keeps the last {keepN}.
                </p>
                <Button
                  variant="secondary"
                  onClick={() => void doBackupNow("proton")}
                  disabled={busy}
                  className="shrink-0"
                >
                  {running ? "Backing up…" : "Back up now"}
                </Button>
              </div>
            ) : (
              <p className="text-xs text-ink4">
                Enter a passphrase under &ldquo;Backup passphrase&rdquo; above, then choose{" "}
                <span className="font-medium">Save to Proton Drive</span> — or set up automatic
                backups below.
              </p>
            )}

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
        <SectionInfo title="How Proton Drive backups work">
          <p>
            Keep your encrypted backups off-machine on your own Proton Drive — end-to-end-encrypted
            cold storage. PM uses Proton&rsquo;s official command-line tool and never sees your
            Proton login.
          </p>
        </SectionInfo>
      </div>

      {/* --- Google Drive (off-machine destination via the Drive API — already connected, so this
          only grants the one extra write permission and reflects status; no install flow) --- */}
      <div className="mt-6">
        <SectionLabel>Google Drive</SectionLabel>
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
                <Select
                  compact
                  className="min-w-0"
                  value={gdriveAccountChoice || gdrive.accounts[0]?.email || ""}
                  onChange={(e) => setGdriveAccountChoice(e.currentTarget.value)}
                  disabled={busy}
                >
                  {gdrive.accounts.map((a) => (
                    <option key={a.email} value={a.email}>
                      {a.email}
                    </option>
                  ))}
                </Select>
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
            {reconcileBanner("gdrive")}
            <div className="flex items-center justify-between gap-2 rounded-[var(--radius-sm)] border border-border2 bg-surface p-3">
              <div className="min-w-0">
                <p className="text-sm text-st-quick">Connected</p>
                {gdrive.account && (
                  <p className="truncate text-xs text-ink4" title={gdrive.account}>
                    {gdrive.account}
                  </p>
                )}
              </div>
              <Button
                variant="tertiary"
                onClick={() => setConfirmDisconnect("gdrive")}
                disabled={busy}
              >
                Disconnect
              </Button>
            </div>
            {passphraseStored ? (
              <div className="flex items-center justify-between gap-2">
                <p className="text-xs text-ink4">
                  Backs up with your remembered passphrase and keeps the last {keepN}.
                </p>
                <Button
                  variant="secondary"
                  onClick={() => void doBackupNow("gdrive")}
                  disabled={busy}
                  className="shrink-0"
                >
                  {running ? "Backing up…" : "Back up now"}
                </Button>
              </div>
            ) : (
              <p className="text-xs text-ink4">
                Enter a passphrase under &ldquo;Backup passphrase&rdquo; above, then choose{" "}
                <span className="font-medium">Save to Google Drive</span> — or set up automatic
                backups below.
              </p>
            )}

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
        <SectionInfo title="How Google Drive backups work">
          <p>
            Keep your encrypted backups on your own Google Drive. They&rsquo;re already encrypted
            before they leave your computer; PM only ever touches its own &ldquo;Personal Manager
            Backups&rdquo; folder (the <span className="font-mono">drive.file</span> permission),
            never the rest of your Drive.
          </p>
        </SectionInfo>
      </div>

      {/* --- Automatic backups — one schedule fans out to every destination you turn on --- */}
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
                Remember your backup passphrase above (under &ldquo;Backup passphrase&rdquo;) to
                turn on automatic backups.
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

      <div className="mt-4">
        <SectionInfo title="How encrypted backup works">
          <p>
            A backup bundles your encrypted database, the Markdown vault, and the vault metadata,
            compresses it with zstd, then encrypts the whole archive with a key stretched from your
            passphrase (Argon2id). The archive is self-contained: it can be restored on a different
            computer with only the passphrase.
          </p>
          <p>
            This is different from <span className="font-medium">Export all data</span> (a plain
            .zip that only opens on this machine) and from{" "}
            <span className="font-medium">Export plaintext Markdown</span> (a readable, unencrypted
            copy of your notes). Keep your passphrase safe — a backup can&rsquo;t be recovered
            without it.
          </p>
        </SectionInfo>
      </div>

      {/* Forgetting the passphrase is the sharpest door on this panel and it was a single unguarded
          click. The dialog names the two things that actually happen rather than asking "are you
          sure": the archives may become unreadable, and the schedule is switched off. It does not
          say "gone forever" — on macOS the keychain entry is still visible in Keychain Access, so
          the true claim is that PM keeps no other copy and cannot show it to you. */}
      <ConfirmDialog
        open={confirmForget}
        title="Forget the passphrase and turn off automatic backups?"
        danger
        confirmLabel="Forget passphrase"
        onConfirm={() => {
          // Close BEFORE awaiting: a keychain failure surfaces through `scheduleSaveError`, which
          // renders outside this dialog, so awaiting first would strand the overlay over it.
          setConfirmForget(false);
          void doForgetPass();
        }}
        onClose={() => setConfirmForget(false)}
      >
        <p>
          PM keeps no other copy of this passphrase and can&rsquo;t show it to you. If it
          isn&rsquo;t written down somewhere else, every backup you&rsquo;ve already made — on this
          computer, Proton Drive and Google Drive — becomes permanently unreadable.
        </p>
        {forgetConsequence && <p className="mt-2">{forgetConsequence}</p>}
        <p className="mt-2">
          Your app lock is a different secret — this doesn&rsquo;t affect getting into PM.
        </p>
      </ConfirmDialog>

      {/* Disconnect, on the pattern every read connector already uses (CloudDriveConnection,
          CalendarConnection, LocalFolderConnection, IcsFeedSubscription): what is KEPT first, what
          stops second, and the per-destination caveat last. This panel was the only one in the app
          holding a Disconnect with no confirmation at all. */}
      <ConfirmDialog
        open={confirmDisconnect !== null}
        title={
          confirmDisconnect === "gdrive"
            ? "Disconnect Google Drive backups?"
            : "Disconnect Proton Drive?"
        }
        danger
        confirmLabel="Disconnect"
        onConfirm={() => {
          const which = confirmDisconnect;
          setConfirmDisconnect(null);
          if (which === "proton") void doProtonDisconnect();
          else if (which === "gdrive") void doGdriveDisconnect();
        }}
        onClose={() => setConfirmDisconnect(null)}
      >
        {confirmDisconnect === "gdrive" ? (
          <>
            <p>
              The backups already on your Google Drive are kept — nothing is deleted. PM stops
              backing up there: scheduled runs and the trimming that keeps only your most recent
              backups stop, and you can&rsquo;t restore from Drive until you grant access again.
            </p>
            {gdriveAlsoConnector ? (
              <p className="mt-2">
                This account is also connected as a read-only source, so its sign-in is kept and
                that connector keeps working.
              </p>
            ) : (
              // Hedged deliberately. Disconnect forgets PM's token WITHOUT revoking the grant at
              // Google's end, so the old per-file authority may or may not survive a re-approval —
              // PM's own Drive code assumes it does not (a 403 appNotAuthorizedToFile on archives
              // an earlier grant uploaded). Neither over-promise nor stay silent about it.
              <p className="mt-2">
                PM&rsquo;s Drive sign-in for this account is deleted. Granting access again runs a
                fresh approval, and Google&rsquo;s permission covers only the files the current
                approval created — so PM may no longer be able to trim or replace the archives it
                uploaded before. They stay in your Drive either way.
              </p>
            )}
          </>
        ) : (
          <>
            <p>
              The backups already on your Proton Drive are kept — nothing is deleted. PM stops
              backing up there: scheduled runs and the trimming that keeps only your most recent
              backups stop, and you can&rsquo;t restore from Proton until you sign in again.
            </p>
            {/* True at HEAD and worth saying: `proton_disconnect` does not clear
                `backup_proton_enabled` (which defaults to true), so the schedule keeps advertising
                a destination the scheduler will skip. Clearing the flag is a backend change and a
                separate decision; telling the truth about it is not. */}
            <p className="mt-2">
              Automatic backups keep listing Proton Drive until you untick it under &ldquo;Automatic
              backups&rdquo; — a scheduled run skips a destination it can&rsquo;t reach.
            </p>
            <p className="mt-2">
              This signs the Proton Drive command-line tool out on this computer, so anything else
              using it is signed out too.
            </p>
          </>
        )}
      </ConfirmDialog>
    </div>
  );
}
