// SPDX-FileCopyrightText: 2026 Bobby Yu
// SPDX-License-Identifier: AGPL-3.0-or-later

import type { AppUpdate } from "../lib/useUpdater";
import { Button } from "./ui";

/**
 * Slim banner shown once a new version has been downloaded in the background.
 * Restarting is always the user's choice — until then the staged update sits
 * quietly and the app keeps working on the current version.
 */
export function UpdateBanner({ update }: { update: AppUpdate }) {
  if (update.status !== "ready" && update.status !== "installing") return null;

  const installing = update.status === "installing";

  // The in-place update couldn't apply (most likely on an unsigned macOS build, where
  // Gatekeeper can refuse the swapped bundle) — point the user at a manual download from
  // the releases page instead of silently looping a failing restart.
  if (!installing && update.installFailed) {
    return (
      <div className="flex items-center justify-between gap-3 border-b border-border bg-accent-soft px-4 py-2 text-sm text-ink2">
        <span>
          Couldn&apos;t install the update automatically
          {update.version ? ` (version ${update.version})` : ""}.
        </span>
        <span className="flex shrink-0 items-center gap-2">
          <a
            href={update.releasesUrl}
            target="_blank"
            rel="noreferrer"
            className="font-medium text-ink underline underline-offset-2 hover:text-ink2"
          >
            Download it manually
          </a>
          <Button variant="tertiary" onClick={update.restart} className="px-2 py-1 text-xs">
            Try again
          </Button>
        </span>
      </div>
    );
  }

  // After "Later", collapse to a slim, always-reachable chip so the staged update
  // isn't lost for the session — the user can still restart whenever they like.
  if (!installing && update.dismissed) {
    return (
      <div className="flex items-center justify-end gap-2 border-b border-border bg-accent-soft px-4 py-1 text-xs text-ink3">
        <span>{update.version ? `Version ${update.version} ready` : "Update ready"}</span>
        <Button variant="tertiary" onClick={update.restart} className="px-2 py-0.5">
          Restart to update
        </Button>
      </div>
    );
  }

  return (
    <div className="flex items-center justify-between gap-3 border-b border-border bg-accent-soft px-4 py-2 text-sm text-ink2">
      <span>
        {installing
          ? "Installing update…"
          : update.version
            ? `Version ${update.version} is ready to install.`
            : "An update is ready to install."}
      </span>
      {!installing && (
        <span className="flex shrink-0 items-center gap-2">
          <Button variant="primary" onClick={update.restart} className="px-2.5 py-1 text-xs">
            Restart now
          </Button>
          <Button variant="tertiary" onClick={update.dismiss} className="px-2 py-1 text-xs">
            Later
          </Button>
        </span>
      )}
    </div>
  );
}
