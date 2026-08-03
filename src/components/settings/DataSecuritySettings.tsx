// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { useEffect, useState } from "react";

import { appLockStatus, dataLocations, openDataFolder, setAppLock } from "../../lib/ipc";
import { IS_LINUX } from "../../lib/setupGuide";
import type { AppLockStatus, DataLocations } from "../../lib/types";
import { ExportDataDialog } from "../ExportDataDialog";
import { RemovePmData } from "../RemovePmData";
import { VaultCard } from "../VaultCard";
import { Button, Callout, SectionInfo, SettingRow, Toggle } from "../ui";

/** The Data & Security Settings tab. Self-contained: the app-lock toggle and the export/reveal
 *  actions each persist/run immediately through their own IPC calls, so there's nothing to batch —
 *  errors surface inline here (the StorageSettings pattern), not in a shared footer. */
export function DataSecuritySettings() {
  const [appLock, setAppLockState] = useState<AppLockStatus | null>(null);
  const [where, setWhere] = useState<DataLocations | null>(null);
  const [exportOpen, setExportOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    appLockStatus()
      .then(setAppLockState)
      .catch(() => {});
    dataLocations()
      .then(setWhere)
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

  return (
    <>
      {error && <Callout className="mt-4">{error}</Callout>}

      <div
        id="sec-data-applock"
        data-settings-section
        className="mt-4 border-t border-border pt-4"
        data-help="settings-app-lock"
      >
        {/* Only the *available* branch of this line was explanation. The other
            two say why the toggle beside them is dead, so they stay inline — as
            does the "can't verify here" notice, which is state, not commentary.
            They are the row's `description`, so they now also name what the dead
            switch is describing (aria-describedby). Both are <span>s inside the
            row's one <p>: a nested <p> is invalid, and the second carries its own
            `mt-1` because the first inherits the paragraph's. */}
        <SettingRow
          label="App lock"
          emphasis="strong"
          spacing="none"
          description={
            !appLock?.available ? (
              <>
                <span className="block">
                  {IS_LINUX
                    ? "Not available on Linux yet. Your store is always encrypted at rest."
                    : "Requires Windows Hello or a configured biometric. Not available on this device yet."}
                </span>
                {appLock?.enabled && (
                  <span className="mt-1 block">
                    App lock is on, but this device can&apos;t verify — PM opens without it here.
                    The setting stays saved and re-arms on a device that can verify.
                  </span>
                )}
              </>
            ) : undefined
          }
        >
          {(a11y) => (
            <Toggle
              {...a11y}
              checked={appLock?.enabled ?? false}
              onChange={(v) => void toggleAppLock(v)}
              disabled={!appLock?.available}
              title={
                appLock?.available
                  ? undefined
                  : IS_LINUX
                    ? "Feature not available on Linux yet"
                    : "Not available on this device"
              }
            />
          )}
        </SettingRow>
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

      {/* Data and Vault were two sections about one folder, and the separation WAS the incident
          (#712): on a Mac the vault path sat as text in one of them while "Open data folder"
          waited about forty lines away in the other — and the two could point at different places,
          because the button opened the profile default while the card showed the resolved root.
          One section now, with the path on the button that opens it. The registry id is the
          existing `sec-data-data`; minting a new one silently breaks the in-rail sub-nav. */}
      <div
        id="sec-data-data"
        data-settings-section
        className="mt-5 border-t border-border pt-4"
        data-help="settings-data"
      >
        <h2 className="block text-sm font-medium text-ink2">Your data</h2>
        {where && (
          <p className="mt-1 break-all text-xs text-ink4">
            Everything PM keeps for you lives in{" "}
            <span className="font-medium text-ink3">{where.vault_root}</span>
          </p>
        )}
        {/* Named only when it differs. A moved or shared vault genuinely has two folders — the
            vault itself, and this profile's own folder holding the pointer to it plus the
            regenerable runtime — and staying silent about the second is how someone ends up backing
            up the wrong one. On an ordinary install there is one place, and it reads as one. */}
        {where?.pointed && (
          <p className="mt-1 break-all text-xs text-ink4">
            PM&rsquo;s own settings folder on this account is separate:{" "}
            <span className="text-ink3">{where.app_data_dir}</span>
          </p>
        )}
        <div className="mt-2 flex flex-wrap gap-2">
          <Button variant="tertiary" onClick={revealDataFolder}>
            Open data folder
          </Button>
          <Button variant="tertiary" onClick={() => setExportOpen(true)}>
            Export&hellip;
          </Button>
        </div>
        <SectionInfo title="About your data & export">
          <p>
            Your documents are Markdown files in that folder, stored unencrypted so any tool can
            read them; PM&rsquo;s own store (projects, chats, the search index) sits beside them and
            is always encrypted. To protect the Markdown when your machine is off or logged out,
            turn on full-disk encryption (BitLocker on Windows, FileVault on macOS, LUKS on Linux).
          </p>
          <p>
            <span className="font-medium">Export</span> offers everything or just your documents,
            plain or encrypted. Plain means readable Markdown — PM&rsquo;s store stays encrypted
            inside the archive either way. Encrypted is the same file a backup writes, and needs a
            passphrase to open.
          </p>
        </SectionInfo>

        <VaultCard />
      </div>

      <ExportDataDialog open={exportOpen} onClose={() => setExportOpen(false)} />

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
