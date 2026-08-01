// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Settings → Backup. This surface owns the whole encrypted-backup feature (deliberately NOT in
// Connectors — a backup is a push-out snapshot, not an index-only source). The tab reads as a
// guided flow: (1) choose the passphrase that locks every backup, optionally remembering it on the
// device for unattended runs; (2) save a backup now — to this computer, Proton Drive, or Google
// Drive; restore from a file or either cloud; and (3) schedule automatic backups on one cadence
// that fans out to every destination you turn on. A status summary up top shows where things stand
// on every launch.
//
// The sections themselves live in `./backup/`. What stays HERE is what more than one of them
// shares, and three of those are load-bearing rather than tidy:
//   * `busy = running || protonBusy || gdriveBusy` — ONE global mutual-exclusion gate (the app-side
//     mirror of the backend's BusyGuard, which is what makes "Disconnect is disabled during an
//     upload it would kill" true), read across six sections. It is computed from state owned by two
//     different destinations, so both destination hooks are instantiated here rather than inside
//     their own components: per-component busy flags would drop the CROSS-destination half of the
//     exclusion silently, and nothing in this repo tests it.
//   * the outcome sinks (`error` / `message` / `warning`) and the restored-vault card — many
//     sections write them, the tab renders each exactly once at the top.
//   * one instance each of the mount snapshot + `backup://progress` subscription, the archive-prefix
//     fetch, and the reconcile-dismissal read. Two of the first would mean two live progress
//     subscriptions handling every phase event twice; two of the others, a doubled IPC round-trip
//     per mount for a value both destinations share.
// The typed passphrase stays in this closure too, and is handed to a destination only at the call
// site that spends it — it is plaintext, and widening its lifetime is the one thing this split
// could have made worse.

import { useEffect, useRef, useState } from "react";
import { save as saveFileDialog } from "@tauri-apps/plugin-dialog";

import {
  backupArchivePrefix,
  backupNow,
  backupStatus,
  clearBackupReport,
  createLocalBackup,
  onBackupProgress,
  pruneOwnBackups,
  setBackupPassphrase,
  setBackupSchedule,
} from "../lib/ipc";
import type { BackupPhase, PassphraseScore, RestoreSummary, RetentionNote } from "../lib/types";
import {
  describeFailures,
  describeForgetConsequences,
  localSaveStamp,
  visibleRetentionNotes,
} from "../lib/backup";
import { readReconcileDismissed, writeReconcileDismissed } from "../lib/backupPrefs";
import { Button, SectionInfo } from "./ui";
import { AutomaticBackupsSection } from "./backup/AutomaticBackupsSection";
import { BackupPassphraseSection } from "./backup/BackupPassphraseSection";
import { BackupRunProgress } from "./backup/BackupRunProgress";
import { BackupStatusSummary } from "./backup/BackupStatusSummary";
import { DisconnectDestinationDialog } from "./backup/DisconnectDestinationDialog";
import { ForgetPassphraseDialog } from "./backup/ForgetPassphraseDialog";
import { GoogleDriveSection } from "./backup/GoogleDriveSection";
import { ProtonDriveSection } from "./backup/ProtonDriveSection";
import { ReconcileBanner } from "./backup/ReconcileBanner";
import { RestoreFromFileSection } from "./backup/RestoreFromFileSection";
import { RestoredVaultCard } from "./backup/RestoredVaultCard";
import { SaveBackupNowSection } from "./backup/SaveBackupNowSection";
import { useBackupSchedule } from "./backup/useBackupSchedule";
import { useGdriveBackup } from "./backup/useGdriveBackup";
import { useProtonBackup } from "./backup/useProtonBackup";

export function BackupSettings() {
  // Live progress (from the global backup://progress event + a status snapshot on mount).
  const [running, setRunning] = useState(false);
  const [phase, setPhase] = useState<BackupPhase | null>(null);
  const [fraction, setFraction] = useState(0);
  // Epoch ms the running backup/restore began, restored from the backend snapshot so leaving and
  // reopening this panel mid-op doesn't restart the elapsed timer. The backend stamps it
  // edge-triggered on idle -> running, so it survives every phase transition.
  const [startedAt, setStartedAt] = useState<number | null>(null);
  // WHICH destination the running backup is for. `BackupEvent` carries no destination (types.ts),
  // and running/phase/fraction are global — so a per-section progress bar needs this or a
  // Proton-only run paints "Uploading" under Google Drive's button too. Set by the call site that
  // started the run, which is the only place that knows.
  const [activeDest, setActiveDest] = useState<"proton" | "gdrive" | "local" | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  // Non-blocking "backed up, but some destinations failed" banner (F-22): distinct from `error`
  // (the whole op failed) and `message` (clean success). Set from a finished run's report.
  const [warning, setWarning] = useState<string | null>(null);
  // Retention trouble is tracked apart from `warning`: those destinations were backed up fine, and
  // unlike a genuine upload failure this half can be re-derived from a fresh listing.
  const [retentionNotes, setRetentionNotes] = useState<RetentionNote[]>([]);

  // The passphrase that locks every backup (manual saves read it directly; "Remember on this
  // device" additionally stows it in the keychain for unattended scheduled runs).
  const [pass, setPass] = useState("");
  const [confirm, setConfirm] = useState("");
  const [passScore, setPassScore] = useState<PassphraseScore | null>(null);

  // The result of a restore, wherever it came from: the file flow and both cloud flows all land
  // here, and the card renders once at the top of the panel rather than under whichever section
  // started it.
  const [restored, setRestored] = useState<RestoreSummary | null>(null);

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

  // The schedule loads first because both destination hooks owe it a reload after a manual backup
  // (it stamps "last backup" too). All four mount loads are independent async IPC calls that write
  // disjoint state, so the order they are issued in is not observable.
  const sched = useBackupSchedule({ setMessage });
  const protonDest = useProtonBackup({
    setError,
    setMessage,
    setRunning,
    setRestored,
    refreshSchedule: sched.refreshSchedule,
  });
  const gdriveDest = useGdriveBackup({
    setError,
    setMessage,
    setRunning,
    setRestored,
    refreshSchedule: sched.refreshSchedule,
  });
  const {
    schedule,
    keepN,
    passphraseStored,
    savingSchedule,
    refreshSchedule,
    setScheduleSaveError,
  } = sched;
  const { conn, protonBackups, protonBusy, setProtonBusy, refreshProton, protonConnected } =
    protonDest;
  const { gdriveBackups, gdriveBusy, setGdriveBusy, refreshGdrive, gdriveGranted } = gdriveDest;

  // The vault's archive prefix is stable for the session — fetch it once. It feeds BOTH
  // destinations' own-archive counts, so it is fetched here rather than inside either of them.
  useEffect(() => {
    backupArchivePrefix()
      .then(setArchivePrefix)
      .catch(() => setArchivePrefix(null));
  }, []);
  // Re-read the banner's dismissal whenever the connected account changes (a different account is a
  // different backup location, so a prior dismissal shouldn't carry over). One effect reading both
  // destinations' accounts and writing one record: splitting it per destination is provably
  // equivalent (the read is a synchronous cache hit) but not identical, so it stays whole.
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
        if (s.last_report) {
          setWarning(describeFailures(s.last_report.failed_destinations));
          setRetentionNotes(s.last_report.retention_notes ?? []);
        }
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
        setRetentionNotes([]);
      } else if (e.type === "finished") {
        setRunning(false);
        setPhase(null);
        setFraction(1);
        setStartedAt(null);
        setActiveDest(null);
        // F-22: some destinations may have failed while others succeeded (a partial success the
        // backend still reports as "finished"). Surface them non-blockingly; null clears cleanly.
        setWarning(describeFailures(e.report.failed_destinations));
        setRetentionNotes(e.report.retention_notes ?? []);
      } else if (e.type === "failed") {
        setRunning(false);
        setPhase(null);
        setStartedAt(null);
        setActiveDest(null);
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
  // The cadence half of what "Forget" costs — null when there is no schedule to lose, so the
  // confirmation never warns about losing something the user hasn't got.
  const forgetConsequence = describeForgetConsequences(schedule?.frequency ?? "off");

  // This vault's own archive count at each destination (the listing includes every vault's archives
  // sharing the account/folder, so filter by our prefix). `null` until both the prefix and the
  // listing have loaded. The reconciliation banner fires when a destination holds more than keep-N.
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

  // Is this destination STILL over its limit? Three answers, and `null` (unknown) is the important
  // one: a count is null while the listing is loading, when the request threw, and when the write
  // scope is missing. Collapsing any of those to "not over the limit" would suppress a true warning
  // exactly when PM can least see the destination, so they stay null and the note stays up.
  function stillOverLimit(kind: string): boolean | null {
    const own = kind === "proton" ? protonOwnCount : kind === "gdrive" ? gdriveOwnCount : null;
    if (own === null || keepN === null) return null;
    return own > keepN;
  }
  // Deleting the extra archives at the destination therefore clears its note on the next visit,
  // with no new IPC and no polling — which is the case that made this banner feel stuck.
  const liveRetentionNotes = visibleRetentionNotes(retentionNotes, stillOverLimit);

  async function dismissLastReport() {
    setWarning(null);
    setRetentionNotes([]);
    // The banner is served from the BACKEND snapshot on every mount, so clearing local state alone
    // brings it straight back on the next tab switch — which is the reported bug.
    await clearBackupReport().catch(() => {});
  }

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

  async function doBackup() {
    setError(null);
    setMessage(null);
    setWarning(null); // a fresh backup supersedes any prior run's partial-failure notice
    let dest: string | null;
    try {
      dest = await saveFileDialog({
        // Stamped like the cloud archives, so a folder of local saves is orderable and a second
        // save doesn't silently offer to overwrite the first. Compact UTC (not the DD-MM-YYYY
        // house format) because a colon-free, lexically sortable name is the whole point here.
        // Nothing parses this string — the user can still rename it in the dialog.
        defaultPath: `personal-manager-backup-${localSaveStamp()}.pmbackup`,
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

  // "Back up now" from a connected panel: uses the STORED passphrase and prunes to keep-last-N (a
  // scheduled-style run), so it only appears once a passphrase is remembered. Distinct from the
  // typed-passphrase "Save to …" buttons in the section above, which never prune.
  async function doBackupNow(kind: "proton" | "gdrive") {
    setError(null);
    setMessage(null);
    try {
      setRunning(true);
      setActiveDest(kind);
      await backupNow(kind);
      setMessage(kind === "proton" ? "Backed up to Proton Drive." : "Backed up to Google Drive.");
      if (kind === "proton") await refreshProton();
      else await refreshGdrive();
      await refreshSchedule();
    } catch (e) {
      setError(String(e));
    } finally {
      setRunning(false);
      setActiveDest(null);
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
  //
  // CARRIED VERBATIM, INCLUDING THE TRAP: this copy is Google-specific but `doPruneOldest` calls it
  // for BOTH destinations. It is unreachable for Proton today only because `proton::apply_retention`
  // hard-codes `skipped: 0`. Inventing Proton wording — even for a branch that cannot run — would be
  // a behaviour change, so it stays wrong-on-purpose and flagged rather than quietly fixed here.
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
  // archives than keep-last-N. All three of its actions are panel-level (raising N is a SCHEDULE
  // write; trimming moves a destination's busy flag into the shared `busy` gate), so it is built
  // here and handed to the destination section as a node.
  function reconcileBanner(kind: "proton" | "gdrive") {
    const present = kind === "proton" ? protonOwnCount : gdriveOwnCount;
    return (
      <ReconcileBanner
        present={present}
        over={kind === "proton" ? protonOverLimit : gdriveOverLimit}
        dismissed={reconcileDismissed[kind]}
        note={pruneNote[kind]}
        keepN={keepN}
        busy={busy}
        destBusy={kind === "proton" ? protonBusy : gdriveBusy}
        onRaiseKeepN={() => void doRaiseKeepN(kind)}
        onPruneOldest={() => void doPruneOldest(kind)}
        onDismiss={() => doDismissReconcile(kind)}
      />
    );
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

      <BackupStatusSummary
        show={sched.showStatus}
        schedule={schedule}
        enabledDestinations={sched.enabledDestinations}
      />

      <BackupRunProgress
        running={running}
        phase={phase}
        fraction={fraction}
        startedAt={startedAt}
      />

      {error && <p className="mt-2 break-words text-xs text-st-due">{error}</p>}
      {/* F-22: a partial success (some destinations failed, at least one succeeded) — a non-blocking
          notice in the shared warning-box idiom, kept separate from `error` (whole op failed).
          Dismiss is required, not decoration: this box is REPLAYED from the backend snapshot on
          every mount, and only a new run ever overwrote it, so a user who repaired the problem
          out-of-band was told about it forever. PM cannot cheaply re-verify an upload failure, so
          acknowledging is the honest primitive — the retention half below heals itself instead. */}
      {(warning || liveRetentionNotes.length > 0) && (
        <div className="mt-2 break-words rounded-[var(--radius)] border px-3 py-2 text-sm text-st-due">
          {warning && <p>{warning}</p>}
          {liveRetentionNotes.map((n) => (
            <p key={`${n.kind}:${n.message}`} className={warning ? "mt-1" : undefined}>
              Backed up. {n.message}
            </p>
          ))}
          <div className="mt-2 flex justify-end">
            <Button variant="tertiary" onClick={() => void dismissLastReport()}>
              Dismiss
            </Button>
          </div>
        </div>
      )}
      {message && <p className="mt-2 break-all text-xs text-st-quick">{message}</p>}

      <RestoredVaultCard restored={restored} setError={setError} />

      {/* --- 1 · Backup passphrase --- */}
      <BackupPassphraseSection
        pass={pass}
        setPass={setPass}
        confirm={confirm}
        setConfirm={setConfirm}
        setPassScore={setPassScore}
        passphraseStored={passphraseStored}
        savingSchedule={savingSchedule}
        busy={busy}
        backupValid={backupValid}
        onRemember={() => void doRememberPass()}
        onForget={() => setConfirmForget(true)}
      />

      {/* --- 2 · Save a backup now --- */}
      <SaveBackupNowSection
        backupValid={backupValid}
        running={running}
        busy={busy}
        protonConnected={protonConnected}
        gdriveGranted={gdriveGranted}
        onSaveLocal={() => void doBackup()}
        onSaveProton={() => void protonDest.doProtonBackup(pass)}
        onSaveGdrive={() => void gdriveDest.doGdriveBackup(pass)}
      />

      {/* --- Restore a backup --- */}
      <RestoreFromFileSection
        running={running}
        setRunning={setRunning}
        setError={setError}
        setMessage={setMessage}
        setRestored={setRestored}
      />

      {/* --- Proton Drive (off-machine destination + automatic backups) --- */}
      <ProtonDriveSection
        state={protonDest}
        busy={busy}
        running={running}
        passphraseStored={passphraseStored}
        keepN={keepN}
        banner={reconcileBanner("proton")}
        onDisconnect={() => setConfirmDisconnect("proton")}
        progress={activeDest === "proton" ? { phase, fraction, startedAt } : null}
        onBackupNow={() => void doBackupNow("proton")}
        onClearRestored={() => setRestored(null)}
      />

      {/* --- Google Drive (off-machine destination via the Drive API — already connected, so this
          only grants the one extra write permission and reflects status; no install flow) --- */}
      <GoogleDriveSection
        state={gdriveDest}
        busy={busy}
        running={running}
        passphraseStored={passphraseStored}
        keepN={keepN}
        banner={reconcileBanner("gdrive")}
        onDisconnect={() => setConfirmDisconnect("gdrive")}
        progress={activeDest === "gdrive" ? { phase, fraction, startedAt } : null}
        onBackupNow={() => void doBackupNow("gdrive")}
        onClearRestored={() => setRestored(null)}
      />

      {/* --- Automatic backups — one schedule fans out to every destination you turn on --- */}
      <AutomaticBackupsSection
        state={sched}
        busy={busy}
        protonConnected={protonConnected}
        gdriveGranted={gdriveGranted}
      />

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

      {/* Both confirmations stay here, as the last two children of the root div. `Modal` does not
          portal, so where a dialog is written is where it lands in the DOM: moved inside a
          destination section, the open Disconnect dialog would put a SECOND button called
          "Disconnect" inside that section. */}
      <ForgetPassphraseDialog
        open={confirmForget}
        forgetConsequence={forgetConsequence}
        onConfirm={() => {
          // Close BEFORE awaiting: a keychain failure surfaces through `scheduleSaveError`, which
          // renders outside this dialog, so awaiting first would strand the overlay over it.
          setConfirmForget(false);
          void sched.doForgetPass();
        }}
        onClose={() => setConfirmForget(false)}
      />

      <DisconnectDestinationDialog
        which={confirmDisconnect}
        gdriveAlsoConnector={gdriveDest.gdriveAlsoConnector}
        onConfirm={() => {
          const which = confirmDisconnect;
          setConfirmDisconnect(null);
          if (which === "proton") void protonDest.doProtonDisconnect();
          else if (which === "gdrive") void gdriveDest.doGdriveDisconnect();
        }}
        onClose={() => setConfirmDisconnect(null)}
      />
    </div>
  );
}
