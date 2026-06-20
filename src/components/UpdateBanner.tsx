// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { AppUpdate } from "../lib/useUpdater";

/**
 * Slim banner shown once a new version has been downloaded in the background.
 * Restarting is always the user's choice — until then the staged update sits
 * quietly and the app keeps working on the current version.
 */
export function UpdateBanner({ update }: { update: AppUpdate }) {
  if (update.status !== "ready" && update.status !== "installing") return null;

  const installing = update.status === "installing";

  // After "Later", collapse to a slim, always-reachable chip so the staged update
  // isn't lost for the session — the user can still restart whenever they like.
  if (!installing && update.dismissed) {
    return (
      <div className="flex items-center justify-end gap-2 border-b border-emerald-900/50 bg-emerald-950/40 px-4 py-1 text-xs text-emerald-300/80">
        <span>{update.version ? `Version ${update.version} ready` : "Update ready"}</span>
        <button
          onClick={update.restart}
          className="rounded px-2 py-0.5 font-medium text-emerald-200 hover:bg-emerald-800/50"
        >
          Restart to update
        </button>
      </div>
    );
  }

  return (
    <div className="flex items-center justify-between gap-3 border-b border-emerald-900 bg-emerald-950/60 px-4 py-2 text-sm text-emerald-200">
      <span>
        {installing
          ? "Installing update…"
          : update.version
            ? `Version ${update.version} is ready to install.`
            : "An update is ready to install."}
      </span>
      {!installing && (
        <span className="flex shrink-0 items-center gap-2">
          <button
            onClick={update.restart}
            className="rounded bg-emerald-700 px-2.5 py-1 text-xs font-medium text-white hover:bg-emerald-600"
          >
            Restart now
          </button>
          <button
            onClick={update.dismiss}
            className="rounded px-2 py-1 text-xs text-emerald-300/80 hover:text-emerald-200"
          >
            Later
          </button>
        </span>
      )}
    </div>
  );
}
