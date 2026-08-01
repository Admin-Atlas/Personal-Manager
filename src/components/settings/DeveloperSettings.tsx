// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { isDevBuild, useDevMode } from "../../lib/capabilities";
import { Button, SectionInfo, SectionLabel, SettingRow, Toggle } from "../ui";

/** The Developer-mode Settings tab. Fully self-contained: `devMode` is a runtime switch that persists
 *  itself through `useDevMode`, so there's nothing to save here. `onOpenDev` jumps to the Dev tab. */
export function DeveloperSettings({ onOpenDev }: { onOpenDev?: () => void }) {
  const { devMode, setDevMode } = useDevMode();
  return (
    <div className="mt-5 border-t border-border pt-4" data-help="settings-developer">
      <SectionLabel>Developer mode</SectionLabel>
      <SettingRow label="Developer mode">
        {(a11y) => <Toggle {...a11y} checked={devMode} onChange={setDevMode} />}
      </SettingRow>
      <div className="mt-3 flex items-center justify-between gap-3 text-xs">
        <span className="text-ink3">Signals</span>
        <span className="font-mono text-ink4">
          build: {isDevBuild ? "dev" : "release"} · runtime: {devMode ? "on" : "off"}
        </span>
      </div>
      {devMode && onOpenDev && (
        <div className="mt-3">
          <Button variant="tertiary" onClick={onOpenDev}>
            Open Dev tab →
          </Button>
        </div>
      )}
      {/* The build/runtime signals above stay put — a readout. Only the paragraph about
          what the switch reveals folds. */}
      <SectionInfo title="What does developer mode reveal?">
        <p>
          Reveals read-only inspection surfaces — a dedicated Dev tab (raw tables, row counts, the
          corrections log, system &amp; build info) plus internals shown in place — for debugging
          and watching how PM works. Strictly read-only: it never changes your data. Independent of
          the density preset, and off by default.
        </p>
      </SectionInfo>
    </div>
  );
}
