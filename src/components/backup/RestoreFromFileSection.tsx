// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

// Restore a backup from a .pmbackup file on this computer. The picked path and its passphrase are
// used nowhere else on the tab, so they live here; the RESULT goes back up to the panel root,
// which renders one restored-vault card for the file flow and both cloud flows alike.

import { useState } from "react";
import { open as openFileDialog } from "@tauri-apps/plugin-dialog";

import type { RestoreSummary } from "../../lib/types";
import { restoreLocalBackup } from "../../lib/ipc";
import { Button, Input, SectionInfo, SectionLabel } from "../ui";

export interface RestoreFromFileSectionProps {
  running: boolean;
  setRunning: (v: boolean) => void;
  setError: (m: string | null) => void;
  setMessage: (m: string | null) => void;
  setRestored: (s: RestoreSummary | null) => void;
}

export function RestoreFromFileSection({
  running,
  setRunning,
  setError,
  setMessage,
  setRestored,
}: RestoreFromFileSectionProps) {
  // Restore form.
  const [restoreSrc, setRestoreSrc] = useState<string | null>(null);
  const [restorePass, setRestorePass] = useState("");

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

  return (
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
                Restore&hellip;
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
  );
}
