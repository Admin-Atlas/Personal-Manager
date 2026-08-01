// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Section 2 · Save a backup now. The three buttons spend the TYPED passphrase, so the handlers are
// bound at the panel root where that draft lives and this component never sees it. (Distinct from
// the "Back up now" inside a connected destination panel, which uses the STORED passphrase and
// prunes to keep-last-N.)

import { Button, SectionInfo, SectionLabel } from "../ui";

export interface SaveBackupNowSectionProps {
  backupValid: boolean;
  running: boolean;
  busy: boolean;
  protonConnected: boolean;
  gdriveGranted: boolean;
  onSaveLocal: () => void;
  onSaveProton: () => void;
  onSaveGdrive: () => void;
}

export function SaveBackupNowSection({
  backupValid,
  running,
  busy,
  protonConnected,
  gdriveGranted,
  onSaveLocal,
  onSaveProton,
  onSaveGdrive,
}: SaveBackupNowSectionProps) {
  return (
    <div className="mt-6">
      <SectionLabel>Save a backup now</SectionLabel>
      <div className="mt-2 flex max-w-sm flex-col gap-2">
        <div className="flex flex-wrap gap-2">
          <Button variant="primary" onClick={onSaveLocal} disabled={!backupValid}>
            Save to this computer…
          </Button>
          {protonConnected && (
            <Button variant="secondary" onClick={onSaveProton} disabled={!backupValid || busy}>
              Save to Proton Drive…
            </Button>
          )}
          {gdriveGranted && (
            <Button variant="secondary" onClick={onSaveGdrive} disabled={!backupValid || busy}>
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
  );
}
