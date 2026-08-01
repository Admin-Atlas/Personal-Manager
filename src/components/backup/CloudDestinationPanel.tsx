// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// The CONNECTED branch of an off-machine destination, written once and rendered by both Proton
// Drive and Google Drive: the reconciliation banner, the Connected card with its Disconnect, the
// "Back up now" row or the passphrase hint that replaces it, the archive listing, and the
// restore-with-passphrase sub-form.
//
// Only this branch is shared. The two setup flows have nothing in common (a CLI install/locate
// flow against an OAuth account picker), and the two status models are different shapes, so those
// stay in `ProtonDriveSection` / `GoogleDriveSection` with their own hooks behind them.

import type { ReactNode } from "react";

import type { BackupEntry } from "../../lib/types";
import { Button, Input } from "../ui";

export interface CloudDestinationPanelProps {
  /** "Proton Drive" / "Google Drive" — also spoken in the archive-list head and the hint copy. */
  destinationName: string;
  account: string | null;
  onDisconnect: () => void;
  /** Already rendered by the caller so the three banner actions stay at the panel root. */
  banner: ReactNode;
  passphraseStored: boolean;
  keepN: number | null;
  running: boolean;
  busy: boolean;
  onBackupNow: () => void;
  listError: string | null;
  onRetryList: () => void;
  backups: BackupEntry[] | null;
  restoreName: string | null;
  setRestoreName: (name: string | null) => void;
  restorePass: string;
  setRestorePass: (pass: string) => void;
  onRestore: () => void;
  /** Clears the shared restored-vault card at the top of the tab. */
  onClearRestored: () => void;
}

export function CloudDestinationPanel({
  destinationName,
  account,
  onDisconnect,
  banner,
  passphraseStored,
  keepN,
  running,
  busy,
  onBackupNow,
  listError,
  onRetryList,
  backups,
  restoreName,
  setRestoreName,
  restorePass,
  setRestorePass,
  onRestore,
  onClearRestored,
}: CloudDestinationPanelProps) {
  return (
    <div className="mt-2 flex max-w-sm flex-col gap-3">
      {banner}
      <div className="flex items-center justify-between gap-2 rounded-[var(--radius-sm)] border border-border2 bg-surface p-3">
        <div className="min-w-0">
          <p className="text-sm text-st-quick">Connected</p>
          {account && (
            <p className="truncate text-xs text-ink4" title={account}>
              {account}
            </p>
          )}
        </div>
        <Button variant="tertiary" onClick={onDisconnect} disabled={busy}>
          Disconnect
        </Button>
      </div>
      {passphraseStored ? (
        <div className="flex items-center justify-between gap-2">
          <p className="text-xs text-ink4">
            Backs up with your remembered passphrase and keeps the last {keepN}.
          </p>
          <Button variant="secondary" onClick={onBackupNow} disabled={busy} className="shrink-0">
            {running ? "Backing up…" : "Back up now"}
          </Button>
        </div>
      ) : (
        <p className="text-xs text-ink4">
          Enter a passphrase under &ldquo;Backup passphrase&rdquo; above, then choose{" "}
          <span className="font-medium">Save to {destinationName}</span> — or set up automatic
          backups below.
        </p>
      )}

      <div>
        <p className="font-mono text-xs uppercase tracking-wide text-ink3">On {destinationName}</p>
        {listError ? (
          <div className="mt-1 flex items-center gap-2">
            <span className="text-xs text-st-due">Couldn&rsquo;t load your backups.</span>
            <Button variant="tertiary" onClick={onRetryList} disabled={busy}>
              Retry
            </Button>
          </div>
        ) : backups === null ? (
          <p className="mt-1 text-xs text-ink4">Loading&hellip;</p>
        ) : backups.length === 0 ? (
          <p className="mt-1 text-xs text-ink4">No backups yet.</p>
        ) : (
          <ul className="mt-1 flex flex-col gap-1">
            {backups.map((b) => (
              <li key={b.name} className="flex items-center justify-between gap-2 text-xs">
                <span className="min-w-0 truncate text-ink3" title={b.name}>
                  {b.name}
                </span>
                <Button
                  variant="tertiary"
                  onClick={() => {
                    setRestoreName(b.name);
                    setRestorePass("");
                    onClearRestored();
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

      {restoreName && (
        <div className="flex flex-col gap-2 rounded-[var(--radius-sm)] border border-border2 bg-surface p-3">
          <p className="text-xs text-ink4">
            Restore <span className="break-all font-medium">{restoreName}</span> — enter its
            passphrase. It unpacks into a new folder; your current vault is untouched until you
            switch.
          </p>
          <Input
            type="password"
            autoComplete="off"
            placeholder="Backup passphrase"
            value={restorePass}
            onChange={(e) => setRestorePass(e.currentTarget.value)}
          />
          <div className="flex gap-2">
            <Button
              variant="primary"
              onClick={onRestore}
              disabled={busy || restorePass.length === 0}
            >
              Restore&hellip;
            </Button>
            <Button
              variant="tertiary"
              onClick={() => {
                setRestoreName(null);
                setRestorePass("");
              }}
            >
              Cancel
            </Button>
          </div>
        </div>
      )}
    </div>
  );
}
