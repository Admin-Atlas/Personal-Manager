// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Google Drive (off-machine destination via the Drive API — already connected, so this only grants
// the one extra write permission and reflects status; no install flow). A three-way state machine
// of which only the connected branch is shared with Proton (`CloudDestinationPanel`).
//
// The `<div className="mt-6">` wrapper belongs to THIS component — see the note in
// `ProtonDriveSection`.

import type { ReactNode } from "react";

import type { BackupPhase } from "../../lib/types";
import { Button, SectionInfo, SectionLabel, Select } from "../ui";
import { CloudDestinationPanel } from "./CloudDestinationPanel";
import type { UseGdriveBackup } from "./useGdriveBackup";

export interface GoogleDriveSectionProps {
  state: UseGdriveBackup;
  /** The panel-wide "any op in flight" gate — `running || protonBusy || gdriveBusy`. */
  busy: boolean;
  running: boolean;
  /** Live progress when the run in flight is THIS destination's; null otherwise. Passed straight
   *  to `CloudDestinationPanel`, which renders the bar beside the button that started it. */
  progress: { phase: BackupPhase | null; fraction: number; startedAt: number | null } | null;
  passphraseStored: boolean;
  keepN: number | null;
  banner: ReactNode;
  onDisconnect: () => void;
  onBackupNow: () => void;
  onClearRestored: () => void;
}

export function GoogleDriveSection({
  state,
  busy,
  running,
  progress,
  passphraseStored,
  keepN,
  banner,
  onDisconnect,
  onBackupNow,
  onClearRestored,
}: GoogleDriveSectionProps) {
  const {
    gdrive,
    gdriveBackups,
    gdriveBusy,
    gdriveRestoreName,
    setGdriveRestoreName,
    gdriveRestorePass,
    setGdriveRestorePass,
    gdriveListError,
    gdriveAccountChoice,
    setGdriveAccountChoice,
    refreshGdrive,
    gdriveGranted,
    doGdriveConnect,
    doGdriveRestore,
  } = state;

  return (
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
        <CloudDestinationPanel
          destinationName="Google Drive"
          account={gdrive.account}
          onDisconnect={onDisconnect}
          banner={banner}
          passphraseStored={passphraseStored}
          keepN={keepN}
          running={running}
          progress={progress}
          busy={busy}
          onBackupNow={onBackupNow}
          listError={gdriveListError}
          onRetryList={() => void refreshGdrive()}
          backups={gdriveBackups}
          restoreName={gdriveRestoreName}
          setRestoreName={setGdriveRestoreName}
          restorePass={gdriveRestorePass}
          setRestorePass={setGdriveRestorePass}
          onRestore={doGdriveRestore}
          onClearRestored={onClearRestored}
        />
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
  );
}
