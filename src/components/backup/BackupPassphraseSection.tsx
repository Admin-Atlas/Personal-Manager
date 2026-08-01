// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Section 1 · Backup passphrase. The draft itself stays in `BackupSettings`' own closure — it is
// plaintext, it is never persisted, and three buttons one section below spend it — so this
// component renders it rather than owning it.

import type { PassphraseScore } from "../../lib/types";
import { Button, Input, SectionInfo, SectionLabel } from "../ui";
import { PassphraseStrengthMeter } from "../PassphraseStrengthMeter";

export interface BackupPassphraseSectionProps {
  pass: string;
  setPass: (v: string) => void;
  confirm: string;
  setConfirm: (v: string) => void;
  setPassScore: (s: PassphraseScore | null) => void;
  passphraseStored: boolean;
  /** A schedule write is in flight — Forget is one, so it must not be re-clickable. */
  savingSchedule: boolean;
  busy: boolean;
  backupValid: boolean;
  onRemember: () => void;
  onForget: () => void;
}

export function BackupPassphraseSection({
  pass,
  setPass,
  confirm,
  setConfirm,
  setPassScore,
  passphraseStored,
  savingSchedule,
  busy,
  backupValid,
  onRemember,
  onForget,
}: BackupPassphraseSectionProps) {
  return (
    <div className="mt-5">
      <SectionLabel>Backup passphrase</SectionLabel>
      <p className="mt-1 text-xs text-ink4">
        This passphrase is the only thing that can unlock a backup later — there&rsquo;s no recovery
        if you lose it, so store it somewhere safe (a password manager).
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
            <Button variant="tertiary" onClick={onForget} disabled={savingSchedule || busy}>
              Forget
            </Button>
          </div>
        ) : (
          <div className="flex flex-col gap-1">
            <div>
              <Button variant="secondary" onClick={onRemember} disabled={!backupValid}>
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
          Choose the passphrase that locks your backups. It&rsquo;s a separate secret from your app
          lock.
        </p>
        <p>
          <span className="font-medium">Remember on this device</span> stores only the passphrase in
          your OS keychain — never your data — so automatic backups can run without asking.
        </p>
      </SectionInfo>
    </div>
  );
}
