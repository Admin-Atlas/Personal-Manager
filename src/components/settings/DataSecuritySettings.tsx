// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useState } from "react";
import { save as saveFileDialog } from "@tauri-apps/plugin-dialog";

import { appLockStatus, exportAllData, openDataFolder, setAppLock } from "../../lib/ipc";
import { IS_LINUX } from "../../lib/setupGuide";
import type { AppLockStatus } from "../../lib/types";
import { RemovePmData } from "../RemovePmData";
import { VaultCard } from "../VaultCard";
import { Button, SectionInfo, Toggle } from "../ui";

/** The Data & Security Settings tab. Self-contained: the app-lock toggle and the export/reveal
 *  actions each persist/run immediately through their own IPC calls, so there's nothing to batch —
 *  errors surface inline here (the StorageSettings pattern), not in a shared footer. */
export function DataSecuritySettings() {
  const [appLock, setAppLockState] = useState<AppLockStatus | null>(null);
  const [exporting, setExporting] = useState(false);
  const [exportMsg, setExportMsg] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    appLockStatus()
      .then(setAppLockState)
      .catch(() => {});
  }, []);

  async function toggleAppLock(next: boolean) {
    setError(null);
    try {
      await setAppLock(next);
      setAppLockState((s) => (s ? { ...s, enabled: next } : s));
    } catch (e) {
      setError(String(e));
    }
  }

  async function revealDataFolder() {
    setError(null);
    try {
      await openDataFolder();
    } catch (e) {
      setError(String(e));
    }
  }

  async function exportData() {
    setError(null);
    setExportMsg(null);
    let dest: string | null;
    try {
      dest = await saveFileDialog({
        defaultPath: "personal-manager-export.zip",
        filters: [{ name: "Zip archive", extensions: ["zip"] }],
      });
    } catch (e) {
      setError(String(e));
      return;
    }
    if (!dest) return; // the user cancelled the dialog
    setExporting(true);
    try {
      await exportAllData(dest);
      setExportMsg(`Exported to ${dest}`);
    } catch (e) {
      setError(String(e));
    } finally {
      setExporting(false);
    }
  }

  return (
    <>
      {error && (
        <div
          className="mt-4 rounded-[var(--radius-sm)] border px-3 py-2 text-xs"
          style={{
            borderColor: "color-mix(in oklab, var(--st-due) 45%, transparent)",
            background: "color-mix(in oklab, var(--st-due) 15%, transparent)",
            color: "var(--st-due)",
          }}
        >
          {error}
        </div>
      )}

      <div
        id="sec-data-applock"
        data-settings-section
        className="mt-4 border-t border-border pt-4"
        data-help="settings-app-lock"
      >
        <div className="flex items-start justify-between gap-3">
          <div>
            <label className="block text-sm font-medium text-ink2">App lock</label>
            {/* Only the *available* branch of this line was explanation. The other
                two say why the toggle beside them is dead, so they stay inline — as
                does the "can't verify here" notice, which is state, not commentary. */}
            {!appLock?.available && (
              <p className="mt-1 text-xs text-ink4">
                {IS_LINUX
                  ? "Not available on Linux yet. Your store is always encrypted at rest."
                  : "Requires Windows Hello or a configured biometric. Not available on this device yet."}
              </p>
            )}
            {appLock?.enabled && !appLock.available && (
              <p className="mt-1 text-xs text-ink4">
                App lock is on, but this device can't verify — PM opens without it here. The setting
                stays saved and re-arms on a device that can verify.
              </p>
            )}
          </div>
          <Toggle
            checked={appLock?.enabled ?? false}
            onChange={(v) => void toggleAppLock(v)}
            ariaLabel="App lock"
            disabled={!appLock?.available}
            title={
              appLock?.available
                ? undefined
                : IS_LINUX
                  ? "Feature not available on Linux yet"
                  : "Not available on this device"
            }
            className="mt-0.5"
          />
        </div>
        {appLock?.available && (
          <SectionInfo title="What does app lock do?">
            <p>
              Require Windows Hello (face, fingerprint, or PIN) to open PM. A convenience lock for
              the window — your store is always encrypted at rest. Takes effect next time you open
              PM.
            </p>
          </SectionInfo>
        )}
      </div>

      <div
        id="sec-data-data"
        data-settings-section
        className="mt-5 border-t border-border pt-4"
        data-help="settings-data"
      >
        <label className="block text-sm font-medium text-ink2">Data</label>
        <div className="mt-2 flex flex-wrap gap-2">
          <Button variant="tertiary" onClick={revealDataFolder}>
            Open data folder
          </Button>
          <Button variant="tertiary" onClick={exportData} disabled={exporting}>
            {exporting ? "Exporting…" : "Export all data…"}
          </Button>
        </div>
        {exportMsg && <p className="mt-2 break-all text-xs text-faint">{exportMsg}</p>}
        <SectionInfo title="About your data & export">
          <p>
            Your documents and the encrypted store live in one folder (
            <span className="font-medium">Personal Manager</span>). Open it to back it up by hand,
            or export everything to a single <span className="font-medium">.zip</span> — the
            Markdown vault plus the encrypted store (the regenerable runtime is left out). The store
            stays encrypted in the archive.
          </p>
          <p>
            Your documents in the Markdown vault are stored unencrypted so any tool can read them.
            To protect them when your machine is off or logged out, turn on full-disk encryption
            (BitLocker on Windows, FileVault on macOS, LUKS on Linux).
          </p>
        </SectionInfo>
      </div>

      <div id="sec-data-vault" data-settings-section>
        <VaultCard />
      </div>

      <div id="sec-data-remove" data-settings-section>
        <RemovePmData biometricAvailable={appLock?.available ?? false} />
      </div>

      <div
        id="sec-data-license"
        data-settings-section
        className="mt-5 border-t border-border pt-4"
        data-help="settings-license"
      >
        <SectionInfo title="License">
          <p>
            PM is free software, licensed under the{" "}
            <a
              href="https://www.gnu.org/licenses/agpl-3.0.html"
              target="_blank"
              rel="noreferrer"
              className="text-ink3 underline hover:text-ink"
            >
              GNU Affero General Public License v3
            </a>
            . © 2026 Bobby Yu.
          </p>
          <p>
            Source code:{" "}
            <a
              href="https://github.com/Admin-Atlas/Personal-Manager"
              target="_blank"
              rel="noreferrer"
              className="text-ink3 underline hover:text-ink"
            >
              github.com/Admin-Atlas/Personal-Manager
            </a>
          </p>
        </SectionInfo>
      </div>
    </>
  );
}
