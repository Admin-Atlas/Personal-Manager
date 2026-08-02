// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Proton Drive (off-machine destination + automatic backups). A five-way state machine — checking
// for the CLI, not installed, checking the session, not connected, connected — of which only the
// last is shared with Google Drive (`CloudDestinationPanel`).
//
// The `<div className="mt-6">` wrapper belongs to THIS component, not to its caller: the tab has
// two buttons called "Disconnect", and the tests scope each one by walking up from its section
// heading to the nearest ancestor `<div>`.

import type { ReactNode } from "react";

import type { BackupPhase } from "../../lib/types";
import { openUrl } from "../../lib/ipc";
import { Button, SectionInfo, SectionLabel } from "../ui";
import { CloudDestinationPanel } from "./CloudDestinationPanel";
import type { UseProtonBackup } from "./useProtonBackup";

export interface ProtonDriveSectionProps {
  state: UseProtonBackup;
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

export function ProtonDriveSection({
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
}: ProtonDriveSectionProps) {
  const {
    proton,
    conn,
    protonBackups,
    protonBusy,
    protonRestoreName,
    setProtonRestoreName,
    protonRestorePass,
    setProtonRestorePass,
    protonListError,
    locateError,
    locating,
    refreshProton,
    locateCli,
    doProtonConnect,
    doProtonRestore,
  } = state;

  return (
    <div className="mt-6">
      <SectionLabel>Proton Drive</SectionLabel>
      {proton === null ? (
        <p className="mt-2 text-xs text-ink4">Checking for the Proton Drive CLI&hellip;</p>
      ) : !proton.installed ? (
        <div className="mt-2 flex max-w-sm flex-col gap-2">
          <p className="text-xs text-ink4">
            The Proton Drive CLI isn&rsquo;t installed. Download the official build — it&rsquo;s a
            single program you can keep anywhere. If it&rsquo;s in your Downloads or on your PATH,
            just <span className="text-ink3">Check again</span>; otherwise point PM straight at it.
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
        <CloudDestinationPanel
          destinationName="Proton Drive"
          account={conn.account}
          onDisconnect={onDisconnect}
          banner={banner}
          passphraseStored={passphraseStored}
          keepN={keepN}
          running={running}
          progress={progress}
          busy={busy}
          onBackupNow={onBackupNow}
          listError={protonListError}
          onRetryList={() => void refreshProton()}
          backups={protonBackups}
          restoreName={protonRestoreName}
          setRestoreName={setProtonRestoreName}
          restorePass={protonRestorePass}
          setRestorePass={setProtonRestorePass}
          onRestore={doProtonRestore}
          onClearRestored={onClearRestored}
        />
      )}
      <SectionInfo title="How Proton Drive backups work">
        <p>
          Keep your encrypted backups off-machine on your own Proton Drive — end-to-end-encrypted
          cold storage. PM uses Proton&rsquo;s official command-line tool and never sees your Proton
          login.
        </p>
      </SectionInfo>
    </div>
  );
}
