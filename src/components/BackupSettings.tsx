// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Settings → Backup. PR1 ships the *local* half of encrypted backup: create a portable,
// passphrase-encrypted `.pmbackup` archive (zstd-compressed, restorable on any machine)
// and restore one into a fresh vault you can switch to. The Proton Drive push/pull and
// scheduling land in later PRs; this surface owns the whole feature (it is deliberately
// NOT in Connectors — a backup is a push-out snapshot, not an index-only source).

import { useEffect, useRef, useState } from "react";
import { open as openFileDialog, save as saveFileDialog } from "@tauri-apps/plugin-dialog";

import {
  backupStatus,
  createLocalBackup,
  onBackupProgress,
  restoreLocalBackup,
  stopBackup,
  switchToVault,
} from "../lib/ipc";
import type { BackupPhase, RestoreSummary } from "../lib/types";
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

  // Backup form.
  const [pass, setPass] = useState("");
  const [confirm, setConfirm] = useState("");

  // Restore form.
  const [restoreSrc, setRestoreSrc] = useState<string | null>(null);
  const [restorePass, setRestorePass] = useState("");
  const [restored, setRestored] = useState<RestoreSummary | null>(null);
  const [switching, setSwitching] = useState(false);

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
  const st = strength(pass);

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
      setMessage(`Encrypted backup saved to ${dest}`);
      setPass("");
      setConfirm("");
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

  return (
    <div className="mt-5 border-t border-border pt-4" data-help="settings-backup">
      <label className="block text-sm font-medium text-ink2">Encrypted backup</label>
      <p className="mt-1 text-sm text-ink3">
        Save a portable, passphrase-encrypted snapshot of your whole vault — restorable on any
        machine. Compressed with zstd, so it&rsquo;s small; encrypted, so it&rsquo;s safe to keep on
        a cloud drive. Backing up to Proton Drive automatically is coming soon.
      </p>

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

      {/* --- Create a backup --- */}
      <div className="mt-4">
        <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
          Create a backup
        </label>
        <p className="mt-1 text-xs text-ink4">
          Choose a passphrase. You&rsquo;ll need it to restore — there is no recovery if you lose
          it, so store it somewhere safe (a password manager).
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
          <div>
            <Button variant="primary" onClick={doBackup} disabled={!backupValid}>
              Create encrypted backup…
            </Button>
          </div>
        </div>
      </div>

      {/* --- Restore a backup --- */}
      <div className="mt-6">
        <label className="block font-mono text-xs font-medium uppercase tracking-wide text-ink3">
          Restore a backup
        </label>
        <p className="mt-1 text-xs text-ink4">
          Restore unpacks the archive into a new folder and checks it before touching anything —
          your current vault is left untouched until you switch to the restored one.
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

        {restored && (
          <div className="mt-3 max-w-sm rounded-[var(--radius-sm)] border border-border2 bg-surface p-3">
            <p className="text-sm text-ink2">Restored a vault, ready to use.</p>
            <p className="mt-1 text-xs text-ink4">
              From a backup made {new Date(restored.created_at).toLocaleString()}. It&rsquo;s in a
              new folder; your current vault is still active until you switch.
            </p>
            <div className="mt-2">
              <Button variant="primary" onClick={doSwitch} disabled={switching}>
                {switching ? "Switching…" : "Switch to the restored vault"}
              </Button>
            </div>
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
