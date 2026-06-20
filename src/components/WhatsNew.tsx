// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import { CHANGELOG } from "../lib/changelog";

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
    <div className="flex h-full items-center justify-center p-6">
      <div className="flex max-h-[80vh] w-full max-w-lg flex-col rounded-xl border border-neutral-800 bg-neutral-900 shadow-xl">
        <div className="flex items-center justify-between border-b border-neutral-800 px-6 py-4">
          <h1 className="text-lg font-semibold text-neutral-100">What's New</h1>
          <button
            onClick={onClose}
            className="rounded-lg px-3 py-1.5 text-sm text-neutral-300 hover:bg-neutral-800"
          >
            Close
          </button>
        </div>

        <div className="flex-1 overflow-y-auto px-6 py-4">
          {CHANGELOG.map((entry) => (
            <section key={entry.version} className="mb-6 last:mb-0">
              <div className="flex items-baseline gap-2">
                <h2 className="text-sm font-semibold text-neutral-100">
                  Version {entry.version}
                </h2>
                {entry.version === currentVersion && (
                  <span className="rounded bg-emerald-900/70 px-1.5 py-0.5 text-[10px] font-medium uppercase tracking-wide text-emerald-300">
                    Current
                  </span>
                )}
                <span className="ml-auto text-xs text-neutral-500">{entry.date}</span>
              </div>
              <ul className="mt-2 space-y-1.5">
                {entry.highlights.map((h, i) => (
                  <li key={i} className="flex gap-2 text-sm text-neutral-300">
                    <span className="select-none text-neutral-600">•</span>
                    <span>{h}</span>
                  </li>
                ))}
              </ul>
            </section>
          ))}
        </div>
      </div>
    </div>
  );
}
