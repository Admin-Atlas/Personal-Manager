// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { CHANGELOG } from "../lib/changelog";
import { Button, Modal } from "./ui";

/**
 * "What's New" — the in-app changelog. Opened automatically once after an update
 * (App.tsx tracks the last version the user has seen) and any time from the
 * sidebar. `currentVersion` is highlighted as the version they're now running.
 */
export function WhatsNew({
  onClose,
  currentVersion,
}: {
  onClose: () => void;
  currentVersion: string | null;
}) {
  return (
    <Modal
      open
      onClose={onClose}
      labelledBy="whats-new-title"
      // Through the height seam, not `className`: passing a rival max-h-* alongside Modal's own left
      // both classes in the list and let stylesheet order pick — so this asked for 80vh and silently
      // got 85vh. The seam replaces the default outright.
      heightClassName="max-h-[80vh]"
      className="flex flex-col"
    >
      <div className="flex items-center justify-between border-b border-border px-6 py-4">
        <h1 id="whats-new-title" className="font-head text-lg font-semibold text-ink">
          What's New
        </h1>
        <Button variant="tertiary" onClick={onClose}>
          Close
        </Button>
      </div>

      <div className="flex-1 overflow-y-auto px-6 py-4">
        {CHANGELOG.map((entry) => (
          <section key={entry.version} className="mb-6 last:mb-0">
            <div className="flex items-baseline gap-2">
              <h2 className="font-head text-sm font-semibold text-ink">Version {entry.version}</h2>
              {entry.version === currentVersion && (
                <span className="rounded-[var(--radius-sm)] bg-accent-soft px-1.5 py-0.5 font-mono text-[10px] font-medium uppercase tracking-wide text-accent-text">
                  Current
                </span>
              )}
              <span className="ml-auto font-mono text-xs text-ink4">{entry.date}</span>
            </div>
            <ul className="mt-2 space-y-1.5">
              {entry.highlights.map((h, i) => (
                <li key={i} className="flex gap-2 text-sm text-ink2">
                  <span className="select-none text-ink4">•</span>
                  <span>{h}</span>
                </li>
              ))}
            </ul>
          </section>
        ))}
      </div>
    </Modal>
  );
}
